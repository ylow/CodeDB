# CodeDB

A code indexing and search system that enables Sourcegraph-style queries against
repositories and their full history. Embedded, built in Rust.

## Features

- **Full-text code search** — literal, regex, phrase, and term search over file contents via Tantivy
- **Diff search** — find commits that introduced or modified specific code
- **Rich SQL queries** — filter by repo, branch, file path, language, commit metadata
- **Unified query engine** — Tantivy search indexes exposed as SQLite virtual tables, enabling JOINs across text search and metadata in a single SQL query
- **Content-addressable dedup** — mirrors git's blob model; identical files across branches/commits are stored and indexed once
- **Incremental updates** — re-indexing is proportional to new commits, not total repo size
- **Embedded** — no external servers; everything runs in-process

## Quick Start

```bash
# Build
cargo build --release

# Index a repository
codedb --root ~/.codedb index https://github.com/user/repo

# Search for code
codedb --root ~/.codedb search "function_name"

# Run arbitrary SQL (with full-text search via virtual tables)
codedb --root ~/.codedb sql "
  SELECT fr.path, cs.score
  FROM code_search('error handling') cs
  JOIN blobs b ON b.id = cs.blob_id
  JOIN file_revs fr ON fr.blob_id = b.id
  JOIN refs r ON r.commit_id = fr.commit_id
  WHERE r.name = 'refs/heads/main'
  ORDER BY cs.score DESC
  LIMIT 10
"
```

## Architecture

```
┌─────────────────────────────────┐
│        codedb-cli (binary)      │
├─────────────────────────────────┤
│        codedb-core (library)    │
│  ┌───────────┐  ┌────────────┐  │
│  │  SQLite   │  │  Tantivy   │  │
│  │ (metadata,│  │ (code_search│  │
│  │  DAG,     │◄─┤  diff_search│  │
│  │  file_revs│  │  via vtab) │  │
│  └───────────┘  └────────────┘  │
│  ┌───────────┐                  │
│  │    gix    │                  │
│  │ (git ops) │                  │
│  └───────────┘                  │
├─────────────────────────────────┤
│     tantivy-sqlite (vtab bridge)│
└─────────────────────────────────┘
```

Three crates in this workspace:

| Crate | Purpose |
|-------|---------|
| `tantivy-sqlite` | Generic SQLite virtual table bridge for Tantivy indexes |
| `codedb-core` | Library: git ingestion, schema, indexing pipeline, search |
| `codedb-cli` | CLI binary wrapping codedb-core |

## Data Directory Layout

```
~/.codedb/
  db.sqlite                          # SQLite database (metadata + virtual tables)
  tantivy/code_search/               # Tantivy index for file contents
  tantivy/diff_search/               # Tantivy index for commit diffs
  repos/{host}/{owner}/{name}.git/   # Bare git clones
```

## Example Queries

```sql
-- Search code on a specific branch
SELECT fr.path, cs.score, cs.snippet
FROM code_search('process_data') cs
JOIN blobs b ON b.id = cs.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
  AND fr.path GLOB '*.rs'
ORDER BY cs.score DESC

-- Find commits that changed code matching a pattern
SELECT c.hash, substr(c.message, 1, 80), ds.score
FROM diff_search('deprecated_function') ds
JOIN diffs d ON d.id = ds.diff_id
JOIN commits c ON c.id = d.commit_id
ORDER BY c.timestamp DESC

-- Language breakdown of a repo
SELECT b.language, COUNT(*) as file_count
FROM blobs b
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
GROUP BY b.language
ORDER BY file_count DESC

-- Recent commits
SELECT hash, author, substr(message, 1, 72) as msg
FROM commits
ORDER BY timestamp DESC
LIMIT 10
```

## Library Usage

```rust
use codedb_core::CodeDB;
use std::path::Path;

let mut db = CodeDB::open(Path::new("/path/to/data"))?;

// Index a repository (clones bare, walks history, populates everything)
db.index_repo("https://github.com/user/repo")?;

// Re-index later (incremental — only processes new commits)
db.index_repo("https://github.com/user/repo")?;

// Query via SQL
let mut stmt = db.conn().prepare(
    "SELECT fr.path, cs.score
     FROM code_search('keyword') cs
     JOIN blobs b ON b.id = cs.blob_id
     JOIN file_revs fr ON fr.blob_id = b.id
     LIMIT 10"
)?;
```

## Demo

Run the included demo script to index [SFrameRust](https://github.com/ylow/SFrameRust/)
and see CodeDB in action:

```bash
./demo.sh
```

It indexes the full repo (117 commits, 381 unique blobs), then runs a series of
queries: database stats, language breakdown, full-text code search, diff search,
file extension filtering, and incremental re-indexing.

## Building

```bash
cargo build --release
```

Requires a working Rust toolchain. All dependencies (SQLite, Tantivy, gix) are compiled from source — no system libraries needed.

## License

MIT
