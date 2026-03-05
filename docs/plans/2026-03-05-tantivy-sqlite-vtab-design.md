# tantivy-sqlite: Generic Tantivy Virtual Table for SQLite

## Summary

A Rust library that exposes any Tantivy index as a read-only SQLite virtual table
via rusqlite. This lets SQL queries join Tantivy full-text search results with
regular SQLite tables, with SQLite's query planner coordinating execution.

## Crate Name

`tantivy-sqlite` (library crate within the CodeDB workspace)

## User-Facing API

### Registration (Builder Pattern)

```rust
use tantivy_sqlite::TantivyVTab;

// User has an existing Tantivy index + reader
let index: tantivy::Index = /* ... */;
let reader: tantivy::IndexReader = /* ... */;

// Register as a SQLite virtual table
TantivyVTab::builder()
    .index(index)
    .reader(reader)
    .search_fields(vec![body_field, title_field])   // fields targeted by query
    .column("doc_id", doc_id_field)                 // Tantivy stored field -> SQL column
    .column("title", title_field)                   // can expose any stored field
    .score_column("score")                          // optional: BM25 score
    .snippet_column("snippet", body_field)          // optional: highlighted snippet
    .register(&conn, "my_search")?;
```

### SQL Usage

```sql
-- Basic: table-valued function style
SELECT doc_id, title, score, snippet
FROM my_search('query terms')

-- With query mode
SELECT doc_id, score FROM my_search('func.*Name', 'regex')

-- Joined with regular tables
SELECT f.path, s.score, s.snippet
FROM my_search('funcName') s
JOIN files f ON f.id = s.doc_id
WHERE f.language = 'python'
ORDER BY s.score DESC
LIMIT 20
```

## Virtual Table Schema

The virtual table is registered as an **eponymous-only** module (table-valued function
pattern). The declared schema has:

1. **User-defined result columns** — mapped from Tantivy stored fields
2. **Built-in score column** (optional) — BM25 relevance score
3. **Built-in snippet column** (optional) — highlighted text fragment
4. **Hidden `query` column** — the search query string (1st positional arg)
5. **Hidden `mode` column** — query mode string (2nd positional arg, default: "default")
6. **Hidden `limit` column** — max results from Tantivy (enables LIMIT pushdown)

Example generated DDL:
```sql
CREATE TABLE my_search(
    doc_id INTEGER,
    title TEXT,
    score REAL,
    snippet TEXT,
    query TEXT HIDDEN,
    mode TEXT HIDDEN,
    query_limit INTEGER HIDDEN
);
```

## Query Modes

| Mode        | Tantivy Query Type      | Example                         |
|-------------|-------------------------|---------------------------------|
| `default`   | QueryParser             | `funcName AND class`            |
| `regex`     | RegexQuery              | `func.*Name`                    |
| `term`      | TermQuery               | `funcName` (exact token match)  |
| `phrase`    | PhraseQuery             | `old man sea`                   |

The `default` mode uses Tantivy's QueryParser which already supports boolean
operators, field-scoped queries, quoted phrases, and prefix wildcards.

## xBestIndex / xFilter Strategy

### Constraint Handling in xBestIndex

The virtual table communicates to SQLite what it can handle:

| Constraint                     | Action                                  |
|--------------------------------|-----------------------------------------|
| `query = ?` (EQ on hidden col)| Required. Pass as argv[0] to xFilter.   |
| `mode = ?` (EQ on hidden col) | Optional. Pass as argv[1] if present.   |
| `query_limit = ?` (EQ)        | Optional. Pass as argv[N] if present.   |
| `LIMIT N`                     | Push down via SQLITE_INDEX_CONSTRAINT_LIMIT if available. |

### idxNum Encoding

Bitmask in idxNum to tell xFilter which args are present:

```
bit 0 (0x01): query is present (always set — required)
bit 1 (0x02): mode is present
bit 2 (0x04): limit is present (from hidden column)
bit 3 (0x08): limit is present (from LIMIT pushdown)
```

### xFilter Execution

1. Parse `idxNum` to determine which args are present.
2. Extract query string from argv[0].
3. Determine mode (argv[1] if present, else "default").
4. Determine limit (argv from hidden col or LIMIT pushdown, else configurable default).
5. Build the appropriate Tantivy query based on mode.
6. Execute search with `TopDocs::with_limit(limit)`.
7. Collect results into a Vec for cursor iteration.

### Cost Estimation

- If query constraint is present: `estimatedCost = 100.0`, `estimatedRows = limit`
- If query constraint is missing: `estimatedCost = 1e18` (force SQLite to never
  scan without a query — the vtab can't return all documents meaningfully)

## Internal Architecture

```
┌──────────────────────────────────────────────┐
│  TantivyVTab::builder()                      │
│    .index(idx).reader(rdr)                   │
│    .search_fields([f1, f2])                  │
│    .column("name", field)                    │
│    .score_column("score")                    │
│    .snippet_column("snip", field)            │
│    .register(&conn, "tbl")?                  │
└────────────┬─────────────────────────────────┘
             │ builds
             ▼
┌──────────────────────────────────────────────┐
│  VTabState (Arc, shared with all cursors)    │
│  - Index, IndexReader                        │
│  - search_fields: Vec<Field>                 │
│  - columns: Vec<ColumnDef>                   │
│  - schema DDL string                         │
│  - default_limit: usize                      │
└────────────┬─────────────────────────────────┘
             │ stored as Aux data
             ▼
┌──────────────────────────────────────────────┐
│  TantivyTable (implements rusqlite VTab)     │
│  - base: sqlite3_vtab                        │
│  - state: Arc<VTabState>                     │
├──────────────────────────────────────────────┤
│  best_index(): examine constraints, set      │
│    argvIndex for query/mode/limit, encode    │
│    idxNum, set cost estimates                │
│  open(): create TantivyCursor                │
└──────────────────────────────────────────────┘
             │ opens
             ▼
┌──────────────────────────────────────────────┐
│  TantivyCursor (implements VTabCursor)       │
│  - base: sqlite3_vtab_cursor                 │
│  - state: Arc<VTabState>                     │
│  - results: Vec<SearchResult>                │
│  - pos: usize                                │
│  - snippet_generator: Option<SnippetGen>     │
├──────────────────────────────────────────────┤
│  filter(): build query, execute search,      │
│    populate results vec                      │
│  next(): pos += 1                            │
│  eof(): pos >= results.len()                 │
│  column(): return field value / score /      │
│    snippet for current result                │
└──────────────────────────────────────────────┘
```

### SearchResult

Each result in the cursor holds:

```rust
struct SearchResult {
    doc_address: DocAddress,
    score: f32,
    // doc is fetched lazily or eagerly depending on which columns are used
}
```

Document field values and snippets are fetched from the Searcher using the
DocAddress. We can either:
- **Eager**: fetch all fields + snippet during filter() and store in the result
- **Lazy**: fetch per-row in column()

Eager is simpler and avoids lifetime issues with the Searcher. The result set is
bounded by the limit so memory is predictable. **Go with eager.**

## Column Type Mapping

| Tantivy Type | SQLite Type | sqlite3_result_* call |
|--------------|-------------|----------------------|
| Str (TEXT)   | TEXT        | sqlite3_result_text   |
| U64          | INTEGER     | sqlite3_result_int64  |
| I64          | INTEGER     | sqlite3_result_int64  |
| F64          | REAL        | sqlite3_result_double |
| Bool         | INTEGER     | sqlite3_result_int    |
| Bytes        | BLOB        | sqlite3_result_blob   |
| Score        | REAL        | sqlite3_result_double |
| Snippet      | TEXT        | sqlite3_result_text   |

## Error Handling

- Builder validates at `.register()` time:
  - At least one search field must be specified
  - All column fields must exist in the Tantivy schema
  - Column fields must be STORED (otherwise we can't retrieve values)
  - Snippet fields must be TEXT type
- Runtime errors in xFilter (bad query syntax, regex parse failure) are
  reported via `sqlite3_vtab.zErrMsg` / rusqlite's error mechanism
- Tantivy panics are caught at the FFI boundary (should not happen, but safety net)

## Thread Safety

- `Index` and `IndexReader` are `Send + Sync` in Tantivy.
- `Searcher` (obtained from reader) is also `Send + Sync`.
- `Arc<VTabState>` shared across table instances and cursors is safe.
- SQLite itself serializes access to a single connection, so concurrent cursor
  access on the same connection is not a concern.

## Testing Strategy

### Unit Tests
- Builder validation: missing fields, invalid column types, duplicate names
- Column type mapping correctness
- idxNum encoding/decoding round-trip

### Integration Tests
- Create an in-memory Tantivy index + SQLite database
- Register the virtual table
- Insert documents into Tantivy
- Run SQL queries and verify results:
  - Basic search returns correct doc IDs and scores
  - Regex mode works
  - Phrase mode works
  - LIMIT pushdown limits Tantivy results
  - Snippet generation produces highlighted text
  - JOIN with a regular SQLite table works correctly
  - Empty result set handled properly
  - Bad query string returns error, not panic
  - Multiple concurrent cursors on same table work
  - Score ordering matches Tantivy native ordering

### Property Tests (stretch goal)
- Roundtrip: any document indexed in Tantivy should be findable via the vtab

## Dependencies

```toml
[dependencies]
tantivy = "0.22"
rusqlite = { version = "0.32", features = ["bundled", "vtab"] }

[dev-dependencies]
tempfile = "3"
```

## Open Design Points

- **Configurable default limit**: What should the default be when no LIMIT is
  specified? 1000? 10000? Should it be set per-registration via the builder?
  Recommend: builder method `.default_limit(1000)` with a sensible default.

- **Stale reader**: If documents are added to the index after the reader was
  created, the vtab won't see them. The user can call `reader.reload()` to
  pick up changes. Should the vtab auto-reload on each query? Recommend: no,
  let the user control it. Expose a method to swap the reader if needed.

- **Multi-value fields**: Tantivy fields can have multiple values per document.
  For now, return the first value. Document this limitation.
