# CodeDB - High Level Design

A code indexing and search system that enables Sourcegraph-style queries against
repositories and their full history. Embedded, built in Rust with C for SQLite
virtual table glue.

## Goals

- Support the Sourcegraph query language: repo, branch, file, language filters;
  literal, regex, and structural text search; symbol lookup; commit and diff search.
- Embedded library (no external server dependencies).
- Efficiently index repositories across all branches and history without
  duplicating unchanged content.

## Architecture Overview

Three distinct workloads, each served by the right tool:

| Workload         | Description                                      | Backend        |
|------------------|--------------------------------------------------|----------------|
| Code search      | Literal, regex, structural search over contents   | Tantivy        |
| Metadata queries | Filter by repo/branch/path/lang, commit DAG       | SQLite         |
| Code intelligence| Symbol defs, refs, call graphs                    | SQLite + SCIP  |

SQLite is the query coordinator. Tantivy is exposed to SQLite via a C virtual
table, allowing the SQLite query planner to build unified execution plans across
both engines.

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
│   │  ┌────────────────┐             │
│   │  │ tantivy_vtab.c │◄─── C FFI  │
│   │  └───────┬────────┘             │
│   │          │                      │
│   └──────────┼──────────┘           │
│              ▼                      │
│   ┌──────────────────┐              │
│   │  Tantivy (Rust)  │              │
│   │  Full-text index │              │
│   └──────────────────┘              │
│                                     │
│   ┌──────────────────┐              │
│   │   gix (Rust)     │              │
│   │   Git ingestion  │              │
│   └──────────────────┘              │
└─────────────────────────────────────┘
```

## Layered Design

### Layer 1: Tantivy Wrapper (Rust, exposes C API)

A Rust library (`cdylib`) wrapping Tantivy with a C-compatible interface.

```c
// C API surface
int tantivy_open(const char *index_path, IndexHandle **out);
int tantivy_close(IndexHandle *handle);
int tantivy_add(IndexHandle *h, int64_t blob_id, const char *content);
int tantivy_search(IndexHandle *h, const char *query, ResultSet **out);
int tantivy_result_next(ResultSet *rs, int64_t *blob_id, float *score);
void tantivy_result_free(ResultSet *rs);
```

### Layer 2: SQLite Virtual Table (C)

A SQLite extension implementing a virtual table backed by the Layer 1 C API.
~200 lines of C. Implements `xBestIndex`, `xFilter`, `xColumn`, `xNext`.

Enables queries like:

```sql
SELECT s.blob_id, s.score, s.snippet
FROM code_search('funcName') s
```

SQLite's query planner can then join this with metadata tables and push down
filters.

### Layer 3: Application (Rust)

- Query parsing (Sourcegraph syntax -> SQL + Tantivy queries)
- Git ingestion via `gix`
- Index management
- Uses `rusqlite` to interact with SQLite (with the vtab extension loaded)

## Data Model

### Content-Addressable Indexing

Files are indexed by content hash (matching git's blob model). A file identical
across 500 commits is stored once in Tantivy and once in SQLite's `blobs` table.

### SQLite Schema

```sql
-- Repositories
CREATE TABLE repos (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE,
    path  TEXT NOT NULL
);

-- Branches and tags
CREATE TABLE refs (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    name      TEXT NOT NULL,       -- e.g. "refs/heads/main"
    commit_id INTEGER NOT NULL REFERENCES commits(id)
);

-- Commits (DAG)
CREATE TABLE commits (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    hash      TEXT NOT NULL,
    author    TEXT,
    message   TEXT,
    timestamp INTEGER
);
CREATE TABLE commit_parents (
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    parent_id INTEGER NOT NULL REFERENCES commits(id)
);

-- Content-addressable blobs
CREATE TABLE blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE   -- git blob SHA
);

-- File instances: a (path, blob) at a specific commit
CREATE TABLE file_revs (
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    path      TEXT NOT NULL,
    language  TEXT,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id)
);

-- Symbols (from tree-sitter or SCIP)
CREATE TABLE symbols (
    id      INTEGER PRIMARY KEY,
    blob_id INTEGER NOT NULL REFERENCES blobs(id),
    name    TEXT NOT NULL,
    kind    TEXT NOT NULL,          -- function, class, variable, etc.
    line    INTEGER,
    col     INTEGER
);

-- Symbol cross-references
CREATE TABLE symbol_refs (
    symbol_id INTEGER NOT NULL REFERENCES symbols(id),
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    line      INTEGER,
    col       INTEGER
);

-- FTS on commit messages
CREATE VIRTUAL TABLE commits_fts USING fts5(message, content=commits, content_rowid=id);
```

### Tantivy Index Schema

One document per unique blob:

| Field       | Type   | Purpose                          |
|-------------|--------|----------------------------------|
| blob_id     | u64    | Foreign key to blobs.id          |
| content     | TEXT   | Full file content (indexed)      |

Metadata filtering (repo, path, language, branch) is handled by SQLite in the
join, not duplicated in Tantivy. This keeps the Tantivy index minimal and avoids
synchronization issues.

### Diff Index

Diffs are indexed as separate Tantivy documents for `type:diff` queries:

| Field       | Type   | Purpose                          |
|-------------|--------|----------------------------------|
| commit_id   | u64    | Foreign key to commits.id        |
| diff_content| TEXT   | Unified diff text (indexed)      |

## Query Flow

Example: `repo:myapp file:\.py$ lang:python funcName`

1. **Parse** Sourcegraph query into filters + text query.
2. **SQLite plans the query:**
   ```sql
   SELECT fr.path, cs.snippet, cs.score
   FROM code_search('funcName') cs
   JOIN blobs b ON b.id = cs.blob_id
   JOIN file_revs fr ON fr.blob_id = b.id
   JOIN commits c ON c.id = fr.commit_id
   JOIN refs r ON r.commit_id = c.id
   JOIN repos rp ON rp.id = c.repo_id
   WHERE rp.name = 'myapp'
     AND fr.path GLOB '*.py'
     AND fr.language = 'python'
     AND r.name = 'refs/heads/main'
   GROUP BY fr.path
   ORDER BY cs.score DESC
   ```
3. SQLite calls into Tantivy via the virtual table for `code_search('funcName')`.
4. SQLite joins the Tantivy results with metadata and returns filtered results.

## Key Dependencies

| Crate/Library | Purpose                              |
|---------------|--------------------------------------|
| `tantivy`     | Full-text and regex search index     |
| `rusqlite`    | SQLite bindings (with `bundled`)     |
| `gix`         | Git repository reading               |
| `tree-sitter` | Source code parsing for symbols      |
| SQLite C API  | Virtual table implementation         |

## Design Decisions

- **Content-addressable dedup**: Mirrors git's blob model. Avoids indexing
  identical file contents across branches/commits.
- **SQLite as query coordinator**: Rather than building a custom federated query
  planner, leverage SQLite's existing optimizer via virtual tables.
- **C virtual table**: SQLite's vtab API is natively C. Writing the thin glue
  layer in C avoids fighting Rust abstraction layers over a C interface.
- **Tantivy for content only**: Metadata stays in SQLite. Tantivy indexes raw
  content. No duplication of repo/path/branch metadata in the search index.
- **Separate diff index**: Diff search and code search have different document
  models and query patterns. Keeping them separate avoids overloading a single
  index.

## Open Questions

- **Incremental indexing**: How to efficiently update the index when a repo
  receives new commits. Walk new objects only via `gix` diffing?
- **file_revs scale**: For repos with deep history, the file_revs table could
  grow large. May need to index only branch tips by default and expand on demand.
- **Regex performance**: Tantivy supports regex but lacks Zoekt's trigram
  optimization. Profile and evaluate whether a trigram pre-filter layer is needed.
- **Structural search**: Sourcegraph's Comby-based structural search. Possibly
  via tree-sitter AST queries as an alternative.
- **Ranking**: How to rank results meaningfully (file importance, symbol
  boundaries, recency, match quality).
