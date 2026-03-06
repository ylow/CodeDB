# Sourcegraph Query Language to SQL Translator Design

## Summary

Add a Sourcegraph-compatible query language frontend to CodeDB. Users write
queries like `lang:rust type:symbol process_data` and CodeDB translates them
to SQL, executes against the existing schema, and returns formatted results.
Supports all four search types: code, diff, commit, and symbol.

## Supported Filters

| Filter | Syntax | Maps to |
|--------|--------|---------|
| `repo:` | `repo:pattern` | `repos.name` substring/GLOB match |
| `file:` / `-file:` | `file:pattern` | `file_revs.path` substring/GLOB match |
| `lang:` | `lang:rust` | `blobs.language` exact match |
| `type:` | `type:symbol\|diff\|commit` | Changes which tables/vtabs are queried |
| `rev:` | `rev:main` | `refs.name` (prefixed with `refs/heads/`) |
| `count:` | `count:50` | SQL `LIMIT` (default 20) |
| `case:` | `case:yes` | Case-sensitive matching |
| `author:` | `author:ylow` | `commits.author` (diff/commit types) |
| `before:` | `before:2026-01-01` | `commits.timestamp` (diff/commit types) |
| `after:` | `after:2025-06-01` | `commits.timestamp` (diff/commit types) |
| `message:` | `message:refactor` | `commits.message` (commit type) |
| `select:` | `select:repo\|file\|symbol.function` | Changes output columns/grouping |
| `calls:` | `calls:groupby` | Find functions that call `groupby` (via `symbol_refs`) |
| `calledby:` | `calledby:groupby` | Find what `groupby` calls (via `symbol_refs`) |

**Not supported:** `fork:`, `archived:`, `visibility:`, `repo:has.*` predicates,
`file:has.owner()`, structural search, `@revision` syntax.

## Pattern Matching

`repo:` and `file:` use **substring match** by default (`LIKE '%pattern%'`).
If the pattern contains `*` or `?`, it is treated as a **GLOB** pattern instead.

Examples:
- `file:csv` matches any path containing "csv"
- `file:*.rs` matches paths ending in `.rs`
- `-file:test` excludes paths containing "test"

## Query Parsing

### Tokenizer

Split input on whitespace. Quoted strings (`"foo bar"`) are kept as single
tokens. Each token is classified as:

- **Filter** — matches `key:value` or `-key:value` for known keys
- **Search term** — everything else, joined into the search pattern

### Parsed Representation

```rust
pub struct ParsedQuery {
    pub search_pattern: String,
    pub search_type: SearchType,
    pub filters: Filters,
}

pub struct Filters {
    pub repo: Option<String>,
    pub file: Option<String>,
    pub neg_file: Option<String>,
    pub lang: Option<String>,
    pub rev: Option<String>,
    pub count: Option<u32>,
    pub case_sensitive: bool,
    pub author: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub message: Option<String>,
    pub select: Option<SelectType>,
}

pub enum SearchType { Code, Diff, Commit, Symbol }
pub enum SelectType { Repo, File, Symbol, SymbolKind(String) }
```

## SQL Generation

### Code Search (default)

```sql
SELECT fr.path, cs.score, cs.snippet
FROM code_search(?1) cs
JOIN blobs b ON b.id = cs.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
JOIN repos rp ON rp.id = r.repo_id          -- only if repo: filter
WHERE 1=1
  AND fr.path LIKE ?2                        -- if file:
  AND fr.path NOT LIKE ?3                    -- if -file:
  AND b.language = ?4                        -- if lang:
  AND r.name = ?5                            -- if rev: (default refs/heads/main)
  AND rp.name LIKE ?6                        -- if repo:
GROUP BY fr.path
ORDER BY cs.score DESC
LIMIT ?7
```

### Diff Search (`type:diff`)

```sql
SELECT substr(c.hash, 1, 10) as hash,
       substr(c.message, 1, 80) as message,
       d.path, round(ds.score, 2) as score
FROM diff_search(?1) ds
JOIN diffs d ON d.id = ds.diff_id
JOIN commits c ON c.id = d.commit_id
WHERE 1=1
  AND c.author LIKE ?2                      -- if author:
  AND c.timestamp < ?3                      -- if before:
  AND c.timestamp > ?4                      -- if after:
  AND d.path LIKE ?5                        -- if file:
ORDER BY ds.score DESC
LIMIT ?6
```

### Commit Search (`type:commit`)

```sql
SELECT substr(c.hash, 1, 10) as hash, c.author,
       substr(c.message, 1, 80) as message
FROM commits c
WHERE c.message LIKE ?1
  AND c.author LIKE ?2                      -- if author:
  AND c.timestamp < ?3                      -- if before:
  AND c.timestamp > ?4                      -- if after:
ORDER BY c.timestamp DESC
LIMIT ?5
```

### Symbol Search (`type:symbol`)

```sql
SELECT fr.path, s.name, s.kind, s.line
FROM symbols s
JOIN blobs b ON b.id = s.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE s.name LIKE ?1
  AND b.language = ?2                       -- if lang:
  AND s.kind = ?3                           -- if select:symbol.function
  AND fr.path LIKE ?4                       -- if file:
  AND r.name = ?5                           -- if rev:
ORDER BY fr.path, s.line
LIMIT ?6
```

### Callers Search (`calls:`)

```sql
SELECT DISTINCT fr.path, s.name, s.kind, s.line
FROM symbol_refs sr
JOIN symbols s ON s.id = sr.symbol_id
JOIN blobs b ON b.id = sr.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE sr.ref_name LIKE ?1                    -- calls: target
  AND sr.kind = 'call'
  AND s.kind = 'function'
  AND r.name = ?2                            -- rev: (default refs/heads/main)
ORDER BY fr.path, s.line
LIMIT ?3
```

### Callees Search (`calledby:`)

```sql
SELECT DISTINCT fr.path, sr.ref_name AS name, sr.kind, sr.line
FROM symbols s
JOIN symbol_refs sr ON sr.symbol_id = s.id AND sr.blob_id = s.blob_id
JOIN blobs b ON b.id = sr.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE s.name LIKE ?1                         -- calledby: source function
  AND s.kind = 'function'
  AND r.name = ?2                            -- rev: (default refs/heads/main)
ORDER BY sr.line
LIMIT ?3
```

### `select:` Modifier

Changes the SELECT/GROUP BY to return distinct values:
- `select:repo` — returns distinct repo names
- `select:file` — returns distinct file paths
- `select:symbol.function` — filters symbols to `kind = 'function'`

### Parameter Binding

All filter values are bound via `?N` placeholders, never interpolated into
SQL strings. This prevents SQL injection.

## CLI Integration

The existing `search` command is upgraded. Backward-compatible — bare terms
still work as plain full-text code search.

```
codedb search "process_data"
codedb search "lang:rust file:*.rs process_data"
codedb search "type:symbol lang:rust SFrame"
codedb search "type:diff author:ylow streaming"
codedb search "type:commit before:2026-01-01 refactor"
codedb search --sql "lang:rust type:symbol foo"
```

### `--sql` Flag

Prints the generated SQL with parameter values shown as comments, without
executing. Useful for debugging and learning.

### Output Formats

| Search type | Output format |
|-------------|--------------|
| Code | `path (score: N.NN)` + snippet |
| Diff | `hash message (score: N.NN)` |
| Commit | `hash author message` |
| Symbol | `path:line kind name` |

## Module Structure

New module: `codedb-core/src/query.rs`

```rust
pub fn parse_query(input: &str) -> Result<ParsedQuery>;
pub fn translate(query: &ParsedQuery) -> TranslatedQuery;

pub struct TranslatedQuery {
    pub sql: String,
    pub params: Vec<String>,
}
```

Public API on CodeDB:

```rust
impl CodeDB {
    pub fn search(&self, query: &str) -> Result<SearchResults>;
    pub fn translate_query(&self, query: &str) -> Result<TranslatedQuery>;
}
```

## Error Handling

Clear error messages for:
- Unknown filter keys
- Invalid `type:` values
- Invalid `count:` values (must be positive integer)
- `before:/after:` date parsing failures
- Missing search pattern when required (code and diff search need a pattern;
  symbol and commit search can work with filters alone)

## Dependencies

None added. Pure string parsing and SQL generation.

## Testing

- **Parser unit tests:** various filter combinations, quoted strings, negation,
  edge cases, error cases
- **SQL generation tests:** verify output SQL for each search type with
  different filter combinations
- **Integration test:** parse, translate, execute against SFrameRust — verify
  results come back for each search type
