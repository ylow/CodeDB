# Tree-sitter Symbol Extraction Design

## Summary

Add tree-sitter-based code intelligence to CodeDB: extract symbol definitions
(functions, methods, classes, structs, etc.) and call references from indexed
blobs. Runs as a separate pass after git indexing. Supports Rust, Python,
JavaScript, TypeScript, Go, C, and C++.

## Schema Changes

Add a `parsed` column to `blobs`:

```sql
CREATE TABLE blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    language     TEXT,
    parsed       INTEGER NOT NULL DEFAULT 0  -- NEW: 1 after successful tree-sitter parse
);
```

The existing `symbols` and `symbol_refs` tables from DESIGN.md are unchanged:

```sql
CREATE TABLE symbols (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    parent_id INTEGER REFERENCES symbols(id),
    name      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL,
    end_line  INTEGER,
    end_col   INTEGER
);

CREATE TABLE symbol_refs (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    symbol_id INTEGER REFERENCES symbols(id),
    ref_name  TEXT NOT NULL,
    kind      TEXT NOT NULL,
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL
);
```

## Architecture

### LanguageConfig

A single struct defines everything needed per language:

```rust
struct LanguageConfig {
    language: tree_sitter::Language,
    extensions: &'static [&'static str],
    def_query: &'static str,        // tree-sitter query for symbol definitions
    ref_query: &'static str,        // tree-sitter query for call references
    scope_node_kinds: &'static [&'static str],
}
```

A `fn get_config(language: &str) -> Option<&LanguageConfig>` maps the
`blobs.language` string (from `detect_language()`) to the right config.

### Generic Processing Function

One function processes all languages identically:

1. Parse source with `tree_sitter::Parser` using the language's grammar
2. Run `def_query` on the AST — collect symbol definitions with name, kind,
   byte range, line/col
3. Sort definitions by byte range — determine `parent_id` via nesting
   (a symbol whose range fully contains another is its parent)
4. Run `ref_query` on the AST — collect call references with name, line/col
5. For each call ref, determine `symbol_id` (containing definition) by
   finding the innermost definition whose range contains the call site
6. Insert into `symbols` and `symbol_refs` tables
7. Set `blobs.parsed = 1`

### Scope Nesting (parent_id)

After collecting all definitions from the def_query, sort them by byte range.
For each definition, scan backwards through previous definitions to find the
innermost one whose range contains it — that's the parent. This is O(n) per
symbol with a stack-based approach:

```
stack = []
for each definition sorted by start position:
    while stack is not empty and stack.top.end <= def.start:
        stack.pop()
    parent = stack.top if not empty, else None
    def.parent_id = parent.id
    stack.push(def)
```

## Pipeline Integration

### Separate Pass

`parse_symbols()` runs after git indexing as a separate pass:

```rust
impl CodeDB {
    pub fn parse_symbols(&mut self) -> Result<()>;
}
```

It queries:
```sql
SELECT b.id, b.content_hash, b.language
FROM blobs b
WHERE b.parsed = 0
  AND b.language IN ('rust', 'python', 'javascript', 'typescript', 'go', 'c', 'cpp')
```

For each unparsed blob:
1. Read content from git object store (try each repo until found)
2. Parse with tree-sitter, extract symbols and refs
3. Insert symbols and symbol_refs
4. Set `parsed = 1`

Wrapped in a SQLite transaction for atomicity.

### CLI Integration

The `index` command calls `parse_symbols()` automatically after `index_repo()`:

```
$ codedb index https://github.com/user/repo
Indexing https://github.com/user/repo...
Done.
Parsing symbols...
Done. Parsed 142 blobs, extracted 1203 symbols.
```

### Incremental Behavior

- Only processes blobs with `parsed = 0`
- Re-running after adding a new language just parses previously-unsupported blobs
- To re-parse everything (e.g., after improving queries): reset `parsed = 0`
  and clear symbols/symbol_refs manually

## Language Configurations

### Rust
- **Definitions**: `function_item`, `struct_item`, `enum_item`, `trait_item`,
  `impl_item` (with type name), `const_item`, `static_item`, `mod_item`
- **Calls**: `call_expression` function name, `macro_invocation` macro name
- **Scope nodes**: `function_item`, `impl_item`, `trait_item`, `mod_item`

### Python
- **Definitions**: `function_definition`, `class_definition`
- **Calls**: `call` function name
- **Scope nodes**: `function_definition`, `class_definition`, `module`

### JavaScript
- **Definitions**: `function_declaration`, `class_declaration`,
  `method_definition`, `arrow_function` (when assigned via `variable_declarator`)
- **Calls**: `call_expression` function name
- **Scope nodes**: `function_declaration`, `class_declaration`, `method_definition`

### TypeScript
- Same as JavaScript plus: `interface_declaration`, `type_alias_declaration`,
  `enum_declaration`
- **Scope nodes**: same as JavaScript plus `interface_declaration`

### Go
- **Definitions**: `function_declaration`, `method_declaration`,
  `type_declaration` (struct/interface), `const_spec`
- **Calls**: `call_expression` function name
- **Scope nodes**: `function_declaration`, `method_declaration`

### C
- **Definitions**: `function_definition`, `struct_specifier`, `enum_specifier`,
  `declaration` (function prototypes)
- **Calls**: `call_expression` function name
- **Scope nodes**: `function_definition`, `struct_specifier`

### C++
- Same as C plus: `class_specifier`, `namespace_definition`
- **Scope nodes**: same as C plus `class_specifier`, `namespace_definition`

## Dependencies

```toml
tree-sitter = "0.24"
tree-sitter-rust = "0.23"
tree-sitter-python = "0.23"
tree-sitter-javascript = "0.23"
tree-sitter-typescript = "0.23"
tree-sitter-go = "0.23"
tree-sitter-c = "0.23"
tree-sitter-cpp = "0.23"
```

Versions will be adjusted to whatever is compatible with `tree-sitter 0.24`.

## Reading Blob Content

`parse_symbols()` needs to read blob content from the git object store.
It opens all repos from the `repos` table and tries each until it finds
the blob by its `content_hash` (git OID). With a small number of repos
per database this is not a performance concern.

## Testing

### Unit Tests
For each language: parse a small inline source snippet, verify correct
symbols (name, kind, parent_id) and refs (ref_name, kind, symbol_id).

Example:
```rust
#[test]
fn test_rust_symbols() {
    let source = r#"
        struct Foo;
        impl Foo {
            fn bar(&self) {
                baz();
            }
        }
    "#;
    let (symbols, refs) = parse_source(source, "rust").unwrap();
    // symbols: Foo (struct), Foo (impl), bar (method, parent=impl Foo)
    // refs: baz (call, symbol_id=bar)
}
```

### Integration Test
After indexing SFrameRust, run `parse_symbols()`, then verify:
- `symbols` table is populated
- `symbol_refs` table is populated
- Query from DESIGN.md works: "what calls X?", "all methods of struct X"

## Query Examples

These queries work unchanged from DESIGN.md:

```sql
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
```
