# CodeDB

A code indexing and search system that enables Sourcegraph-style queries against
repositories and their full history. Embedded, built in Rust.

## Features

- **Sourcegraph-style search** — intuitive query language with filters like `repo:`, `lang:`, `file:`, `type:`, `calls:`, `returns:`
- **Full-text code search** — literal, regex, phrase, and term search over file contents via Tantivy
- **Diff search** — find commits that introduced or modified specific code
- **Commit search** — search commit metadata by author, date range, and message
- **Symbol search** — find functions, structs, classes, traits, and other symbols via tree-sitter
- **Cross-reference queries** — `calls:fn` to find callers, `calledby:fn` to find callees
- **Type-aware search** — `returns:Type` to find functions by return type
- **Rich SQL queries** — filter by repo, branch, file path, language, commit metadata
- **Unified query engine** — Tantivy search indexes exposed as SQLite virtual tables, enabling JOINs across text search and metadata in a single SQL query
- **Content-addressable dedup** — mirrors git's blob model; identical files across branches/commits are stored and indexed once
- **Incremental updates** — re-indexing is proportional to new commits, not total repo size
- **Embedded** — no external servers; everything runs in-process

## Quick Start

```bash
# Build
cargo build --release

# Index a repository (clones, walks history, extracts symbols)
codedb --root ~/.codedb index https://github.com/user/repo

# Search for code (Sourcegraph-style query)
codedb --root ~/.codedb search "function_name"

# Filtered search
codedb --root ~/.codedb search "lang:rust file:*.rs -file:test serialize"

# Find symbols
codedb --root ~/.codedb search "type:symbol select:symbol.function SFrame"

# Cross-reference: who calls groupby()?
codedb --root ~/.codedb search "calls:groupby"

# Type info: functions returning BatchIterator
codedb --root ~/.codedb search "returns:BatchIterator"

# Diff search: commits that touched "streaming"
codedb --root ~/.codedb search "type:diff file:*.rs streaming"

# Commit search by author
codedb --root ~/.codedb search "type:commit author:Yucheng parallel"

# Show generated SQL instead of executing
codedb --root ~/.codedb search --sql "lang:rust file:*.rs serialize"

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

## Search Query Syntax

The `search` command uses a Sourcegraph-compatible query language.
Bare words are search terms; filters use `key:value` syntax.

### Filters

| Filter | Description | Example |
|--------|-------------|---------|
| `repo:` | Filter by repository name | `repo:SFrameRust` |
| `file:` / `-file:` | Include / exclude file paths | `file:*.rs -file:test` |
| `lang:` or `l:` | Filter by language | `lang:rust` |
| `type:` | Search type: `code`, `diff`, `commit`, `symbol` | `type:symbol` |
| `rev:` | Branch or ref (default: `refs/heads/main`) | `rev:develop` |
| `select:` | Output format: `repo`, `file`, `symbol`, `symbol.KIND` | `select:symbol.function` |
| `count:` | Max results (default: 20) | `count:50` |
| `case:` | Case sensitivity (`yes`/`no`) | `case:yes` |
| `author:` | Commit/diff author | `author:Yucheng` |
| `before:` / `after:` | Date range for commits/diffs | `after:2024-01-01` |
| `message:` | Commit message filter | `message:refactor` |
| `calls:` | Find functions that call a given function | `calls:groupby` |
| `calledby:` | Find functions called by a given function | `calledby:groupby` |
| `returns:` | Find functions returning a given type | `returns:SFrame` |

### Search Types

- **`type:code`** (default) — Full-text search across file contents. Returns path, score, and snippet.
- **`type:symbol`** — Search extracted symbols. Use `select:symbol.KIND` to filter by kind (function, struct, class, etc.).
- **`type:diff`** — Search within commit diffs. Supports `author:`, `before:`, `after:`, `file:` filters.
- **`type:commit`** — Search commit metadata. Supports `author:`, `before:`, `after:`, `message:` filters.

## Symbol Extraction

CodeDB uses tree-sitter to extract symbols from source code during indexing.

### Supported Languages

| Language | Symbol Types |
|----------|-------------|
| Rust | function, struct, enum, trait, impl, const, static, module |
| Python | function, class |
| JavaScript | function, class, method, interface, enum, type_alias |
| TypeScript | function, class, method, interface, enum, type_alias |
| TSX | function, class, method, interface, enum, type_alias |
| Go | function, method, type |
| C | function, struct, enum |
| C++ | function, struct, enum, class, namespace |

### What's Extracted

- **Symbols** — name, kind, location (line/column), full signature
- **Type info** — return types, parameter lists
- **Scope nesting** — methods within classes/impls tracked via parent relationships
- **Call references** — function call sites with containing symbol context

## Architecture

```
┌─────────────────────────────────────┐
│        codedb-cli (binary)          │
├─────────────────────────────────────┤
│        codedb-core (library)        │
│  ┌───────────┐  ┌────────────────┐  │
│  │  SQLite   │  │    Tantivy     │  │
│  │ (metadata,│  │ (code_search,  │  │
│  │  DAG,     │◄─┤  diff_search   │  │
│  │  file_revs│  │  via vtab)     │  │
│  └───────────┘  └────────────────┘  │
│  ┌───────────┐  ┌────────────────┐  │
│  │    gix    │  │  tree-sitter   │  │
│  │ (git ops) │  │ (symbols,      │  │
│  │           │  │  call refs)    │  │
│  └───────────┘  └────────────────┘  │
├─────────────────────────────────────┤
│     tantivy-sqlite (vtab bridge)    │
└─────────────────────────────────────┘
```

Three crates in this workspace:

| Crate | Purpose |
|-------|---------|
| `tantivy-sqlite` | Generic SQLite virtual table bridge for Tantivy indexes |
| `codedb-core` | Library: git ingestion, schema, indexing, symbol extraction, query translation |
| `codedb-cli` | CLI binary wrapping codedb-core |

## Database Schema

| Table | Description |
|-------|-------------|
| `repos` | Indexed repositories |
| `commits` | Commit metadata (hash, author, message, timestamp) |
| `commit_parents` | Commit parent relationships (DAG) |
| `refs` | Branch/tag refs pointing to commits |
| `blobs` | Unique file contents (content-addressable by SHA) |
| `file_revs` | Files present at each ref tip |
| `diffs` | Per-file diffs for each commit |
| `symbols` | Extracted symbols (name, kind, signature, return type, params) |
| `symbol_refs` | Call sites and references between symbols |
| `code_search()` | Virtual table — full-text search over file contents |
| `diff_search()` | Virtual table — full-text search over diffs |

## Data Directory Layout

```
~/.codedb/
  db.sqlite                          # SQLite database (metadata + virtual tables)
  tantivy/code_search/               # Tantivy index for file contents
  tantivy/diff_search/               # Tantivy index for commit diffs
  repos/{host}/{owner}/{name}.git/   # Bare git clones
```

## Example SQL Queries

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

-- Most called functions (excluding common builtins)
SELECT sr.ref_name AS function, COUNT(*) AS calls
FROM symbol_refs sr
JOIN blobs b ON b.id = sr.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
  AND sr.kind = 'call'
GROUP BY sr.ref_name
ORDER BY calls DESC
LIMIT 15

-- Functions with specific parameter types
SELECT DISTINCT fr.path || ':' || s.line AS location, s.params
FROM symbols s
JOIN blobs b ON b.id = s.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE s.params LIKE '%SFrame%'
  AND s.kind = 'function'
  AND r.name = 'refs/heads/main'

-- Language breakdown of a repo
SELECT b.language, COUNT(*) as file_count
FROM blobs b
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
GROUP BY b.language
ORDER BY file_count DESC
```

## Library Usage

```rust
use codedb_core::CodeDB;
use std::path::Path;

let mut db = CodeDB::open(Path::new("/path/to/data"))?;

// Index a repository (clones bare, walks history, extracts symbols)
db.index_repo("https://github.com/user/repo")?;

// Re-index later (incremental — only processes new commits)
db.index_repo("https://github.com/user/repo")?;

// Sourcegraph-style search
let results = db.search("lang:rust type:symbol SFrame")?;

// Or query via SQL directly
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

It indexes the full repo, then runs a series of queries demonstrating:
database stats, language breakdown, full-text code search, symbol search,
cross-reference queries (calls/calledby), type-aware queries (returns),
diff search, commit search, SQL generation, and incremental re-indexing.

## Building

```bash
cargo build --release
```

Requires a working Rust toolchain. All dependencies (SQLite, Tantivy, gix,
tree-sitter) are compiled from source — no system libraries needed.

## License

MIT
