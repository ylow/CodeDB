# CodeDB - High Level Design

A code indexing and search system that enables Sourcegraph-style queries against
repositories and their full history. Embedded, built in Rust.

## Goals

- Support the Sourcegraph query language: repo, branch, file, language filters;
  literal, regex, and structural text search; symbol lookup; commit and diff search.
- Embedded library (no external server dependencies).
- Efficiently index repositories across all branches and history without
  duplicating unchanged content.
- Incremental updates: re-indexing after new commits should be proportional to
  the number of new commits, not the size of the repository.

## Architecture Overview

Three distinct workloads, each served by the right tool:

| Workload          | Description                                     | Backend          |
|-------------------|-------------------------------------------------|------------------|
| Code search       | Literal, regex search over file contents         | Tantivy          |
| Diff search       | Search within commit diffs                       | Tantivy          |
| Metadata queries  | Filter by repo/branch/path/lang, commit DAG      | SQLite           |
| Code intelligence | Symbol defs, refs, call graphs (tree-sitter)     | SQLite           |

SQLite is the query coordinator. Tantivy is exposed to SQLite via a virtual
table (the `tantivy-sqlite` crate), allowing the SQLite query planner to build
unified execution plans across both engines.

```
┌─────────────────────────────────────┐
│          Application (Rust)         │
│                                     │
│   Sourcegraph query parser          │
│          │                          │
│          ▼                          │
│   ┌─────────────┐                   │
│   │   SQLite     │                   │
│   │  (rusqlite)  │                   │
│   │              │                   │
│   │  ┌──────────────────────┐       │
│   │  │ tantivy-sqlite vtab  │       │
│   │  └──────────┬───────────┘       │
│   │             │                   │
│   └─────────────┼───────────┘       │
│                 ▼                   │
│   ┌──────────────────┐              │
│   │  Tantivy (Rust)  │              │
│   │  code_search     │              │
│   │  diff_search     │              │
│   └──────────────────┘              │
│                                     │
│   ┌──────────────────┐              │
│   │  gix (Rust)      │              │
│   │  Git ingestion   │              │
│   └──────────────────┘              │
│                                     │
│   ┌──────────────────┐              │
│   │  tree-sitter     │              │
│   │  Symbol parsing  │              │
│   └──────────────────┘              │
└─────────────────────────────────────┘
```

## Data Model

### Core Principles

- **Content-addressable dedup**: Mirrors git's blob model. A file identical
  across 500 commits is stored and parsed once. Blob identity = git SHA.
- **Append-only data**: Blobs, symbols, symbol_refs, commits, and diffs are
  immutable once written. Incremental updates only append.
- **Copy-on-write file trees**: When a branch tip moves, the new tip's file
  tree is derived from the old tip's tree plus the accumulated diffs.
- **Branch-tip indexing**: file_revs are stored for branch/tag tips, not every
  commit. Arbitrary commit lookups fall back to gix.

### SQLite Schema

```sql
-- ============ Git layer ============

CREATE TABLE repos (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL
);

CREATE TABLE refs (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    name      TEXT NOT NULL,
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    UNIQUE(repo_id, name)
);

CREATE TABLE commits (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    hash      TEXT NOT NULL UNIQUE,
    author    TEXT,
    message   TEXT,
    timestamp INTEGER
);

CREATE TABLE commit_parents (
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    parent_id INTEGER NOT NULL REFERENCES commits(id),
    PRIMARY KEY (commit_id, parent_id)
);

-- ============ Content layer ============

CREATE TABLE blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,  -- git blob SHA
    language     TEXT                   -- detected at index time
);

-- File tree snapshot at specific commits (typically branch/tag tips).
-- When a ref moves, the new tip's file_revs are derived by copying
-- the old tip's file_revs and applying the diffs.
CREATE TABLE file_revs (
    id        INTEGER PRIMARY KEY,
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    path      TEXT NOT NULL,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    UNIQUE(commit_id, path)
);

-- What changed in each commit (relative to first parent).
CREATE TABLE diffs (
    id          INTEGER PRIMARY KEY,
    commit_id   INTEGER NOT NULL REFERENCES commits(id),
    path        TEXT NOT NULL,
    old_blob_id INTEGER REFERENCES blobs(id),  -- NULL for added files
    new_blob_id INTEGER REFERENCES blobs(id),  -- NULL for deleted files
    UNIQUE(commit_id, path)
);

-- ============ Symbol layer (tree-sitter) ============

-- Symbol definitions within a blob. Parsed once per unique blob.
CREATE TABLE symbols (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    parent_id INTEGER REFERENCES symbols(id),  -- scope nesting (method → class)
    name      TEXT NOT NULL,
    kind      TEXT NOT NULL,  -- function, method, class, struct, interface,
                              -- variable, constant, module, etc.
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL,
    end_line  INTEGER,
    end_col   INTEGER
);

-- Syntactic references within a blob. "Symbol S appears to call/reference
-- name N." Resolution is name-based (tree-sitter cannot resolve across files).
CREATE TABLE symbol_refs (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    symbol_id INTEGER REFERENCES symbols(id),  -- containing symbol (caller)
    ref_name  TEXT NOT NULL,                    -- the name being referenced
    kind      TEXT NOT NULL,                    -- call, import, type_ref
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL
);

-- ============ Indexes ============

CREATE INDEX idx_commits_repo ON commits(repo_id);
CREATE INDEX idx_refs_repo ON refs(repo_id);
CREATE INDEX idx_file_revs_commit ON file_revs(commit_id);
CREATE INDEX idx_file_revs_blob ON file_revs(blob_id);
CREATE INDEX idx_diffs_commit ON diffs(commit_id);
CREATE INDEX idx_symbols_blob ON symbols(blob_id);
CREATE INDEX idx_symbols_name ON symbols(name);
CREATE INDEX idx_symbol_refs_blob ON symbol_refs(blob_id);
CREATE INDEX idx_symbol_refs_name ON symbol_refs(ref_name);
CREATE INDEX idx_symbol_refs_symbol ON symbol_refs(symbol_id);
```

### Tantivy Indexes

**Code search index** — one document per unique blob:

| Field    | Type | Purpose                     |
|----------|------|-----------------------------|
| blob_id  | u64  | Foreign key to blobs.id     |
| content  | TEXT | Full file content (indexed) |

**Diff search index** — one document per diff entry:

| Field        | Type | Purpose                     |
|--------------|------|-----------------------------|
| diff_id      | u64  | Foreign key to diffs.id     |
| diff_content | TEXT | Unified diff text (indexed) |

Both indexes are registered as SQLite virtual tables via `tantivy-sqlite`,
enabling queries like:

```sql
SELECT blob_id, score, snippet FROM code_search('funcName')
SELECT diff_id, score, snippet FROM diff_search('removed_function')
```

## Incremental Update Algorithm

```
update_repo(repo):
  1. gix: fetch current refs, compare with stored refs
  2. For each ref that moved (old_commit → new_commit):
     a. Walk commits from new_commit back to old_commit
     b. For each new commit:
        - Insert into commits, commit_parents
        - Compute diff from parent → insert into diffs
        - Index diff text in Tantivy diff_search
        - For any new blobs in the diff:
          → Insert blob, index content in Tantivy code_search
          → Parse with tree-sitter → insert symbols, symbol_refs
     c. Compute file_revs for new tip:
        - Copy old tip's file_revs
        - Apply accumulated diffs (add/remove/modify paths)
        - Insert as new tip commit_id's file_revs
  3. Update refs table with new tip commit_ids
  4. Delete refs that no longer exist
  5. Optional: GC orphaned blobs, file_revs for unreachable commits
```

Key properties:
- Work is proportional to new commits, not total repo size
- Blobs, symbols, symbol_refs are append-only (never modified)
- file_revs for old tips remain valid (historical snapshots)

## Query Examples

```sql
-- Text search on a branch
SELECT fr.path, cs.score, cs.snippet
FROM code_search('funcName') cs
JOIN blobs b ON b.id = cs.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
JOIN repos rp ON rp.id = r.repo_id
WHERE rp.name = 'myapp'
  AND fr.path GLOB '*.py'
  AND r.name = 'refs/heads/main'
GROUP BY fr.path
ORDER BY cs.score DESC

-- What functions call process_data?
SELECT DISTINCT fr.path, s.name, s.kind, sr.line
FROM symbol_refs sr
JOIN symbols s ON s.id = sr.symbol_id
JOIN file_revs fr ON fr.blob_id = sr.blob_id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE sr.ref_name = 'process_data'
  AND sr.kind = 'call'
  AND r.name = 'refs/heads/main'

-- What does function foo call?
SELECT DISTINCT sr.ref_name, sr.kind, sr.line
FROM symbols s
JOIN symbol_refs sr ON sr.symbol_id = s.id
WHERE s.name = 'foo' AND s.kind = 'function'

-- All methods of struct Output
SELECT s.name, s.line
FROM symbols s
JOIN symbols parent ON s.parent_id = parent.id
WHERE parent.name = 'Output' AND parent.kind = 'struct'
  AND s.kind = 'method'

-- Search diffs
SELECT c.hash, c.message, ds.snippet
FROM diff_search('removed_function') ds
JOIN diffs d ON d.id = ds.diff_id
JOIN commits c ON c.id = d.commit_id
ORDER BY c.timestamp DESC

-- Transitive callers (up to 5 hops)
WITH RECURSIVE callers(name, depth, path) AS (
    SELECT s.name, 1, fr.path
    FROM symbol_refs sr
    JOIN symbols s ON s.id = sr.symbol_id
    JOIN file_revs fr ON fr.blob_id = sr.blob_id
    JOIN refs r ON r.commit_id = fr.commit_id
    WHERE sr.ref_name = 'process_data' AND sr.kind = 'call'
      AND r.name = 'refs/heads/main'
    UNION
    SELECT s.name, callers.depth + 1, fr.path
    FROM symbol_refs sr
    JOIN symbols s ON s.id = sr.symbol_id
    JOIN file_revs fr ON fr.blob_id = sr.blob_id
    JOIN refs r ON r.commit_id = fr.commit_id
    JOIN callers ON sr.ref_name = callers.name
    WHERE sr.kind = 'call' AND callers.depth < 5
      AND r.name = 'refs/heads/main'
)
SELECT DISTINCT name, path, depth FROM callers ORDER BY depth
```

## Key Dependencies

| Crate/Library  | Purpose                              |
|----------------|--------------------------------------|
| `tantivy`      | Full-text and regex search index     |
| `rusqlite`     | SQLite bindings (with `bundled`)     |
| `tantivy-sqlite` | Tantivy ↔ SQLite virtual table bridge |
| `gix`          | Git repository reading               |
| `tree-sitter`  | Source code parsing for symbols      |

## Design Decisions

- **Content-addressable dedup**: Mirrors git's blob model. Avoids indexing
  identical file contents across branches/commits.
- **SQLite as query coordinator**: Rather than building a custom federated query
  planner, leverage SQLite's existing optimizer via virtual tables.
- **Pure Rust vtab**: The `tantivy-sqlite` crate implements the virtual table
  using rusqlite's vtab traits, avoiding a separate C layer.
- **Tantivy for content only**: Metadata stays in SQLite. Tantivy indexes raw
  content and diffs. No duplication of repo/path/branch metadata in the search
  index.
- **Tree-sitter for code intelligence**: Syntactic, intra-file, name-based
  symbol extraction and reference detection. No SCIP/LSP dependency. Cross-file
  reference resolution is heuristic (name matching).
- **Branch-tip file trees**: file_revs stored at ref tips, not every commit.
  Keeps table size proportional to (branches × files) not (commits × files).
- **Append-only core**: Blobs, symbols, commits, diffs are immutable once
  written. Incremental updates only append new data.

## Open Questions

- **Regex performance**: Tantivy supports regex but lacks Zoekt's trigram
  optimization. Profile and evaluate whether a trigram pre-filter layer is needed.
- **Structural search**: Sourcegraph's Comby-based structural search. Possibly
  via tree-sitter AST queries as an alternative.
- **Ranking**: How to rank results meaningfully (file importance, symbol
  boundaries, recency, match quality).
- **Language detection**: Currently on blobs table for simplicity, but language
  depends on file path (extension), not content. May need to revisit if the
  same blob appears at paths with different extensions.
