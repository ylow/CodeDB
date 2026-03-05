# Tree-sitter Symbol Extraction Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add tree-sitter-based symbol extraction and call reference detection to CodeDB, enabling "find definition", "what calls X?", and "all methods of struct X" queries.

**Architecture:** A new `symbols.rs` module with a `LanguageConfig` struct per language and one generic processing function. Runs as a separate pass after git indexing, processing blobs with `parsed = 0`. Each language provides tree-sitter query strings for definitions and call references; the generic function handles parsing, scope nesting via byte ranges, and DB insertion.

**Tech Stack:** tree-sitter 0.26, tree-sitter-rust/python/javascript/typescript/go/c/cpp grammars, rusqlite

---

### Task 1: Add tree-sitter dependencies and update schema

**Files:**
- Modify: `codedb-core/Cargo.toml`
- Modify: `codedb-core/src/schema.rs`

**Step 1: Add dependencies to `codedb-core/Cargo.toml`**

Add under `[dependencies]`:
```toml
tree-sitter = "0.26"
tree-sitter-rust = "0.24"
tree-sitter-python = "0.25"
tree-sitter-javascript = "0.25"
tree-sitter-typescript = "0.23"
tree-sitter-go = "0.25"
tree-sitter-c = "0.24"
tree-sitter-cpp = "0.23"
```

**Step 2: Update schema — add `parsed` column to blobs table**

In `codedb-core/src/schema.rs`, change the blobs table definition from:
```sql
CREATE TABLE IF NOT EXISTS blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    language     TEXT
);
```
to:
```sql
CREATE TABLE IF NOT EXISTS blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    language     TEXT,
    parsed       INTEGER NOT NULL DEFAULT 0
);
```

**Step 3: Verify it compiles and tests pass**

Run: `cargo test -p codedb-core --lib`
Expected: All unit tests pass (schema tests should still work since we use IF NOT EXISTS and the new column has a default)

**Step 4: Commit**

```bash
git add codedb-core/Cargo.toml codedb-core/src/schema.rs
git commit -m "feat: add tree-sitter dependencies and parsed column to blobs"
```

---

### Task 2: LanguageConfig struct and language registry

**Files:**
- Create: `codedb-core/src/symbols.rs`
- Modify: `codedb-core/src/lib.rs`

**Step 1: Create the LanguageConfig struct and registry**

`codedb-core/src/symbols.rs`:

```rust
use tree_sitter::Language;

/// Configuration for tree-sitter symbol extraction for a single language.
pub(crate) struct LanguageConfig {
    pub language: Language,
    pub def_query: &'static str,
    pub ref_query: &'static str,
}

/// Return the LanguageConfig for a language string (as stored in blobs.language).
/// Returns None for unsupported languages.
pub(crate) fn get_config(language: &str) -> Option<LanguageConfig> {
    match language {
        "rust" => Some(rust_config()),
        "python" => Some(python_config()),
        "javascript" => Some(javascript_config()),
        "typescript" => Some(typescript_config()),
        "tsx" => Some(tsx_config()),
        "go" => Some(go_config()),
        "c" => Some(c_config()),
        "cpp" => Some(cpp_config()),
        _ => None,
    }
}

/// List of language names that have tree-sitter support.
pub(crate) fn supported_languages() -> &'static [&'static str] {
    &["rust", "python", "javascript", "typescript", "tsx", "go", "c", "cpp"]
}

fn rust_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_rust::LANGUAGE.into(),
        def_query: "
            (function_item name: (identifier) @name) @def
            (struct_item name: (type_identifier) @name) @def
            (enum_item name: (type_identifier) @name) @def
            (trait_item name: (type_identifier) @name) @def
            (impl_item type: (type_identifier) @name) @def
            (const_item name: (identifier) @name) @def
            (static_item name: (identifier) @name) @def
            (mod_item name: (identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (field_expression field: (field_identifier) @ref_name)) @ref
            (call_expression function: (scoped_identifier name: (identifier) @ref_name)) @ref
            (macro_invocation macro: (identifier) @ref_name) @ref
        ",
    }
}

fn python_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_python::LANGUAGE.into(),
        def_query: "
            (function_definition name: (identifier) @name) @def
            (class_definition name: (identifier) @name) @def
        ",
        ref_query: "
            (call function: (identifier) @ref_name) @ref
            (call function: (attribute attribute: (identifier) @ref_name)) @ref
        ",
    }
}

fn javascript_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_javascript::LANGUAGE.into(),
        def_query: "
            (function_declaration name: (identifier) @name) @def
            (class_declaration name: (identifier) @name) @def
            (method_definition name: (property_identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (member_expression property: (property_identifier) @ref_name)) @ref
        ",
    }
}

fn typescript_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        def_query: "
            (function_declaration name: (identifier) @name) @def
            (class_declaration name: (type_identifier) @name) @def
            (method_definition name: (property_identifier) @name) @def
            (interface_declaration name: (type_identifier) @name) @def
            (enum_declaration name: (identifier) @name) @def
            (type_alias_declaration name: (type_identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (member_expression property: (property_identifier) @ref_name)) @ref
        ",
    }
}

fn tsx_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_typescript::LANGUAGE_TSX.into(),
        // Same queries as TypeScript
        def_query: "
            (function_declaration name: (identifier) @name) @def
            (class_declaration name: (type_identifier) @name) @def
            (method_definition name: (property_identifier) @name) @def
            (interface_declaration name: (type_identifier) @name) @def
            (enum_declaration name: (identifier) @name) @def
            (type_alias_declaration name: (type_identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (member_expression property: (property_identifier) @ref_name)) @ref
        ",
    }
}

fn go_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_go::LANGUAGE.into(),
        def_query: "
            (function_declaration name: (identifier) @name) @def
            (method_declaration name: (field_identifier) @name) @def
            (type_declaration (type_spec name: (type_identifier) @name)) @def
            (const_declaration (const_spec name: (identifier) @name)) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (selector_expression field: (field_identifier) @ref_name)) @ref
        ",
    }
}

fn c_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_c::LANGUAGE.into(),
        def_query: "
            (function_definition declarator: (function_declarator declarator: (identifier) @name)) @def
            (struct_specifier name: (type_identifier) @name) @def
            (enum_specifier name: (type_identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
        ",
    }
}

fn cpp_config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_cpp::LANGUAGE.into(),
        def_query: "
            (function_definition declarator: (function_declarator declarator: (identifier) @name)) @def
            (function_definition declarator: (function_declarator declarator: (qualified_identifier name: (identifier) @name))) @def
            (struct_specifier name: (type_identifier) @name) @def
            (class_specifier name: (type_identifier) @name) @def
            (enum_specifier name: (type_identifier) @name) @def
            (namespace_definition name: (identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (field_expression field: (field_identifier) @ref_name)) @ref
            (call_expression function: (qualified_identifier name: (identifier) @ref_name)) @ref
        ",
    }
}
```

IMPORTANT: Tree-sitter query syntax is finicky. The queries above are based on
each grammar's node structure. If a query fails to compile at runtime, the error
message will tell you what went wrong. Common fixes:
- Check `tree.root_node().to_sexp()` to see the actual AST structure
- Use `playground.tree-sitter.io` to test queries (select the right grammar)
- Some grammars use `type_identifier` vs `identifier` for type names

**Step 2: Update lib.rs**

Add `pub(crate) mod symbols;` to `codedb-core/src/lib.rs`.

**Step 3: Verify it compiles**

Run: `cargo build -p codedb-core`
Expected: Compiles (may take a while downloading grammars). If any query string
fails to parse at runtime, that's OK — we'll fix those in Task 3 with tests.

**Step 4: Commit**

```bash
git add codedb-core/src/symbols.rs codedb-core/src/lib.rs
git commit -m "feat: add LanguageConfig struct and language registry for tree-sitter"
```

---

### Task 3: Generic symbol extraction function with Rust unit tests

**Files:**
- Modify: `codedb-core/src/symbols.rs`

**Step 1: Add the generic extraction function and types**

Add to `codedb-core/src/symbols.rs` (above the language config functions):

```rust
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

/// A symbol definition extracted from source code.
#[derive(Debug, Clone)]
pub(crate) struct ExtractedSymbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub parent_index: Option<usize>, // index into the symbols vec
}

/// A call reference extracted from source code.
#[derive(Debug, Clone)]
pub(crate) struct ExtractedRef {
    pub ref_name: String,
    pub kind: String,
    pub line: usize,
    pub col: usize,
    pub start_byte: usize,
    pub containing_symbol_index: Option<usize>, // index into the symbols vec
}

/// Parse source code and extract symbols and references.
/// Returns (symbols, refs) or None if parsing fails.
pub(crate) fn extract_symbols(
    source: &str,
    config: &LanguageConfig,
) -> Option<(Vec<ExtractedSymbol>, Vec<ExtractedRef>)> {
    let mut parser = Parser::new();
    parser.set_language(&config.language).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let source_bytes = source.as_bytes();

    // 1. Extract definitions
    let def_query = Query::new(&config.language, config.def_query).ok()?;
    let def_name_idx = def_query.capture_index_for_name("name")?;
    let def_idx = def_query.capture_index_for_name("def")?;

    let mut symbols = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&def_query, root, source_bytes);

    while let Some(m) = matches.next() {
        let mut name_text = None;
        let mut kind_text = None;
        let mut def_node = None;

        for capture in m.captures {
            if capture.index == def_name_idx {
                name_text = capture.node.utf8_text(source_bytes).ok();
            }
            if capture.index == def_idx {
                def_node = Some(capture.node);
                kind_text = Some(capture.node.kind());
            }
        }

        if let (Some(name), Some(kind), Some(node)) = (name_text, kind_text, def_node) {
            let start = node.start_position();
            let end = node.end_position();
            symbols.push(ExtractedSymbol {
                name: name.to_string(),
                kind: normalize_kind(kind),
                line: start.row + 1,     // 1-based
                col: start.column + 1,   // 1-based
                end_line: end.row + 1,
                end_col: end.column + 1,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                parent_index: None,
            });
        }
    }

    // 2. Determine parent_id via byte range nesting
    //    Sort by start_byte, then use a stack to find parents
    symbols.sort_by_key(|s| (s.start_byte, std::cmp::Reverse(s.end_byte)));
    let mut scope_stack: Vec<usize> = Vec::new(); // stack of symbol indices
    for i in 0..symbols.len() {
        while let Some(&top) = scope_stack.last() {
            if symbols[top].end_byte <= symbols[i].start_byte {
                scope_stack.pop();
            } else {
                break;
            }
        }
        symbols[i].parent_index = scope_stack.last().copied();
        scope_stack.push(i);
    }

    // 3. Extract call references
    let ref_query = Query::new(&config.language, config.ref_query).ok()?;
    let ref_name_idx = ref_query.capture_index_for_name("ref_name")?;
    let ref_idx = ref_query.capture_index_for_name("ref")?;

    let mut refs = Vec::new();
    let mut cursor2 = QueryCursor::new();
    let mut ref_matches = cursor2.matches(&ref_query, root, source_bytes);

    while let Some(m) = ref_matches.next() {
        let mut name_text = None;
        let mut ref_node = None;

        for capture in m.captures {
            if capture.index == ref_name_idx {
                name_text = capture.node.utf8_text(source_bytes).ok();
            }
            if capture.index == ref_idx {
                ref_node = Some(capture.node);
            }
        }

        if let (Some(name), Some(node)) = (name_text, ref_node) {
            let start = node.start_position();
            let call_byte = node.start_byte();

            // Find containing symbol: last symbol in sorted order whose range
            // contains this call site
            let containing = symbols.iter().enumerate().rev().find(|(_, s)| {
                s.start_byte <= call_byte && call_byte < s.end_byte
            }).map(|(idx, _)| idx);

            refs.push(ExtractedRef {
                ref_name: name.to_string(),
                kind: "call".to_string(),
                line: start.row + 1,
                col: start.column + 1,
                start_byte: call_byte,
                containing_symbol_index: containing,
            });
        }
    }

    Some((symbols, refs))
}

/// Normalize tree-sitter node kinds to consistent symbol kinds.
fn normalize_kind(ts_kind: &str) -> String {
    match ts_kind {
        "function_item" | "function_definition" | "function_declaration" => "function".to_string(),
        "method_definition" | "method_declaration" => "method".to_string(),
        "struct_item" | "struct_specifier" => "struct".to_string(),
        "class_definition" | "class_declaration" | "class_specifier" => "class".to_string(),
        "enum_item" | "enum_specifier" | "enum_declaration" => "enum".to_string(),
        "trait_item" => "trait".to_string(),
        "impl_item" => "impl".to_string(),
        "interface_declaration" => "interface".to_string(),
        "type_alias_declaration" | "type_declaration" => "type".to_string(),
        "const_item" | "const_declaration" | "const_spec" => "constant".to_string(),
        "static_item" => "static".to_string(),
        "mod_item" => "module".to_string(),
        "namespace_definition" => "namespace".to_string(),
        "macro_invocation" => "macro_call".to_string(),
        other => other.to_string(),
    }
}
```

**Step 2: Add unit tests for Rust extraction**

Add at the bottom of `codedb-core/src/symbols.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_function_def() {
        let source = r#"
fn hello() {
    println!("hello");
}

fn world() {
    hello();
}
"#;
        let config = get_config("rust").unwrap();
        let (symbols, refs) = extract_symbols(source, &config).unwrap();

        // Should find two functions
        let func_names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(func_names.contains(&"hello"), "symbols: {:?}", func_names);
        assert!(func_names.contains(&"world"), "symbols: {:?}", func_names);

        // Both should be kind "function"
        for s in &symbols {
            assert_eq!(s.kind, "function");
            assert!(s.parent_index.is_none()); // top-level
        }

        // Should find hello() call inside world()
        let call_names: Vec<&str> = refs.iter().map(|r| r.ref_name.as_str()).collect();
        assert!(call_names.contains(&"hello"), "refs: {:?}", call_names);
    }

    #[test]
    fn test_rust_struct_and_impl() {
        let source = r#"
struct Foo;

impl Foo {
    fn bar(&self) {
        baz();
    }

    fn qux(&self) {}
}
"#;
        let config = get_config("rust").unwrap();
        let (symbols, refs) = extract_symbols(source, &config).unwrap();

        // Find Foo struct and Foo impl
        let foo_struct = symbols.iter().find(|s| s.name == "Foo" && s.kind == "struct");
        assert!(foo_struct.is_some(), "symbols: {:?}", symbols);

        let foo_impl = symbols.iter().find(|s| s.name == "Foo" && s.kind == "impl");
        assert!(foo_impl.is_some(), "symbols: {:?}", symbols);

        // bar and qux should have parent pointing to the impl
        let bar = symbols.iter().find(|s| s.name == "bar").unwrap();
        assert!(bar.parent_index.is_some(), "bar should have parent");
        let parent = &symbols[bar.parent_index.unwrap()];
        assert_eq!(parent.name, "Foo");

        // baz() call should be inside bar
        let baz_ref = refs.iter().find(|r| r.ref_name == "baz").unwrap();
        assert!(baz_ref.containing_symbol_index.is_some());
        let containing = &symbols[baz_ref.containing_symbol_index.unwrap()];
        assert_eq!(containing.name, "bar");
    }

    #[test]
    fn test_python_extraction() {
        let source = r#"
class MyClass:
    def method(self):
        other_func()

def standalone():
    pass
"#;
        let config = get_config("python").unwrap();
        let (symbols, refs) = extract_symbols(source, &config).unwrap();

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MyClass"), "symbols: {:?}", names);
        assert!(names.contains(&"method"), "symbols: {:?}", names);
        assert!(names.contains(&"standalone"), "symbols: {:?}", names);

        // method should be inside MyClass
        let method = symbols.iter().find(|s| s.name == "method").unwrap();
        assert!(method.parent_index.is_some());
        let parent = &symbols[method.parent_index.unwrap()];
        assert_eq!(parent.name, "MyClass");

        // other_func() call
        let call = refs.iter().find(|r| r.ref_name == "other_func");
        assert!(call.is_some(), "refs: {:?}", refs);
    }

    #[test]
    fn test_go_extraction() {
        let source = r#"
package main

func hello() {
    world()
}

func world() {}
"#;
        let config = get_config("go").unwrap();
        let (symbols, refs) = extract_symbols(source, &config).unwrap();

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"), "symbols: {:?}", names);
        assert!(names.contains(&"world"), "symbols: {:?}", names);

        let call = refs.iter().find(|r| r.ref_name == "world");
        assert!(call.is_some(), "refs: {:?}", refs);
    }

    #[test]
    fn test_c_extraction() {
        let source = r#"
struct Point {
    int x;
    int y;
};

int add(int a, int b) {
    return a + b;
}

int main() {
    add(1, 2);
    return 0;
}
"#;
        let config = get_config("c").unwrap();
        let (symbols, refs) = extract_symbols(source, &config).unwrap();

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"), "symbols: {:?}", names);
        assert!(names.contains(&"add"), "symbols: {:?}", names);
        assert!(names.contains(&"main"), "symbols: {:?}", names);

        let call = refs.iter().find(|r| r.ref_name == "add");
        assert!(call.is_some(), "refs: {:?}", refs);
    }

    #[test]
    fn test_unsupported_language() {
        assert!(get_config("fortran").is_none());
    }

    #[test]
    fn test_supported_languages() {
        let langs = supported_languages();
        assert!(langs.contains(&"rust"));
        assert!(langs.contains(&"python"));
        assert!(langs.contains(&"go"));
        assert!(langs.contains(&"c"));
        assert!(langs.contains(&"cpp"));
        assert!(langs.contains(&"javascript"));
        assert!(langs.contains(&"typescript"));
        assert!(langs.contains(&"tsx"));
    }
}
```

**Step 3: Run tests and iterate**

Run: `cargo test -p codedb-core --lib symbols`
Expected: All tests pass. If tree-sitter queries fail to compile at runtime,
fix the query strings based on the error messages. Common issues:
- Wrong capture name or node kind for a given grammar
- Missing field names (check with `tree.root_node().to_sexp()`)

**Step 4: Commit**

```bash
git add codedb-core/src/symbols.rs
git commit -m "feat: implement generic symbol extraction with tree-sitter queries"
```

---

### Task 4: parse_symbols() function — DB integration

**Files:**
- Modify: `codedb-core/src/symbols.rs`
- Modify: `codedb-core/src/codedb.rs`

**Step 1: Add the parse_symbols function**

Add to `codedb-core/src/symbols.rs`:

```rust
use anyhow::{Context, Result};
use rusqlite::Connection;

/// Parse symbols for all unparsed blobs that have a supported language.
/// Reads blob content from git repos in the repos table.
pub fn parse_symbols(conn: &Connection, repos_dir: &std::path::Path) -> Result<ParseStats> {
    // Build the IN clause for supported languages
    let langs = supported_languages();
    let placeholders: Vec<String> = langs.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let in_clause = placeholders.join(", ");

    // Query unparsed blobs with supported languages
    let query = format!(
        "SELECT id, content_hash, language FROM blobs WHERE parsed = 0 AND language IN ({in_clause})"
    );
    let mut stmt = conn.prepare(&query)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = langs.iter().map(|l| l as &dyn rusqlite::types::ToSql).collect();
    let rows: Vec<(i64, String, String)> = stmt
        .query_map(params.as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;

    if rows.is_empty() {
        return Ok(ParseStats { blobs_parsed: 0, symbols_extracted: 0 });
    }

    // Open all repos for reading blob content
    let mut repos = Vec::new();
    {
        let mut repo_stmt = conn.prepare("SELECT path FROM repos")?;
        let paths: Vec<String> = repo_stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for path in paths {
            if let Ok(repo) = gix::open(std::path::Path::new(&path)) {
                repos.push(repo);
            }
        }
    }

    let tx = conn;
    tx.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<ParseStats> {
        let mut total_symbols = 0u64;
        let mut total_blobs = 0u64;

        for (blob_id, content_hash, language) in &rows {
            let config = match get_config(language) {
                Some(c) => c,
                None => continue,
            };

            // Read blob content from git repos
            let oid = gix::ObjectId::from_hex(content_hash.as_bytes())
                .context("Invalid content_hash")?;
            let content = repos.iter().find_map(|repo| {
                repo.find_object(oid).ok().and_then(|obj| {
                    String::from_utf8(obj.data.clone()).ok()
                })
            });

            let content = match content {
                Some(c) => c,
                None => {
                    // Can't read blob — mark parsed to avoid retrying
                    tx.execute("UPDATE blobs SET parsed = 1 WHERE id = ?1", rusqlite::params![blob_id])?;
                    continue;
                }
            };

            // Parse with tree-sitter
            match extract_symbols(&content, &config) {
                Some((symbols, refs)) => {
                    // Insert symbols
                    let mut symbol_db_ids: Vec<i64> = Vec::with_capacity(symbols.len());
                    for sym in &symbols {
                        tx.execute(
                            "INSERT INTO symbols (blob_id, parent_id, name, kind, line, col, end_line, end_col)
                             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7)",
                            rusqlite::params![
                                blob_id, sym.name, sym.kind,
                                sym.line, sym.col, sym.end_line, sym.end_col
                            ],
                        )?;
                        symbol_db_ids.push(tx.last_insert_rowid());
                    }

                    // Update parent_id for symbols that have parents
                    for (i, sym) in symbols.iter().enumerate() {
                        if let Some(parent_idx) = sym.parent_index {
                            let parent_db_id = symbol_db_ids[parent_idx];
                            let sym_db_id = symbol_db_ids[i];
                            tx.execute(
                                "UPDATE symbols SET parent_id = ?1 WHERE id = ?2",
                                rusqlite::params![parent_db_id, sym_db_id],
                            )?;
                        }
                    }

                    // Insert refs
                    for r in &refs {
                        let symbol_id = r.containing_symbol_index.map(|idx| symbol_db_ids[idx]);
                        tx.execute(
                            "INSERT INTO symbol_refs (blob_id, symbol_id, ref_name, kind, line, col)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![
                                blob_id, symbol_id, r.ref_name, r.kind, r.line, r.col
                            ],
                        )?;
                    }

                    total_symbols += symbols.len() as u64;
                    total_blobs += 1;
                }
                None => {
                    // Parse failed — still mark as parsed to avoid retrying
                }
            }

            tx.execute("UPDATE blobs SET parsed = 1 WHERE id = ?1", rusqlite::params![blob_id])?;
        }

        Ok(ParseStats {
            blobs_parsed: total_blobs,
            symbols_extracted: total_symbols,
        })
    })();

    match result {
        Ok(stats) => {
            tx.execute_batch("COMMIT")?;
            Ok(stats)
        }
        Err(e) => {
            let _ = tx.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

pub struct ParseStats {
    pub blobs_parsed: u64,
    pub symbols_extracted: u64,
}
```

Also add `use gix;` at the top of the file.

**Step 2: Add `parse_symbols()` to CodeDB**

In `codedb-core/src/codedb.rs`, add:

```rust
use crate::symbols;

// Inside impl CodeDB:
pub fn parse_symbols(&self) -> Result<symbols::ParseStats> {
    symbols::parse_symbols(self.conn(), &self.repos_dir())
}
```

Make `ParseStats` public by adding `pub use symbols::ParseStats;` in `lib.rs`,
or just keep it `pub` in symbols.rs — the CLI needs to read the fields.

**Step 3: Run unit tests**

Run: `cargo test -p codedb-core --lib`
Expected: All tests pass

**Step 4: Commit**

```bash
git add codedb-core/src/symbols.rs codedb-core/src/codedb.rs codedb-core/src/lib.rs
git commit -m "feat: implement parse_symbols() with DB integration"
```

---

### Task 5: Wire parse_symbols into CLI

**Files:**
- Modify: `codedb-cli/src/main.rs`

**Step 1: Call parse_symbols after index_repo in the Index command**

Change the `Commands::Index` arm from:
```rust
Commands::Index { url } => {
    let mut db = CodeDB::open(&root)?;
    println!("Indexing {url}...");
    db.index_repo(&url)?;
    println!("Done.");
}
```

to:
```rust
Commands::Index { url } => {
    let mut db = CodeDB::open(&root)?;
    println!("Indexing {url}...");
    db.index_repo(&url)?;
    println!("Parsing symbols...");
    let stats = db.parse_symbols()?;
    println!("Done. Parsed {} blobs, extracted {} symbols.", stats.blobs_parsed, stats.symbols_extracted);
}
```

**Step 2: Build**

Run: `cargo build -p codedb-cli`
Expected: Compiles

**Step 3: Commit**

```bash
git add codedb-cli/src/main.rs
git commit -m "feat: wire parse_symbols into CLI index command"
```

---

### Task 6: Integration test with SFrameRust

**Files:**
- Modify: `codedb-core/tests/integration.rs`

**Step 1: Add symbol extraction integration test**

Read the existing `codedb-core/tests/integration.rs` first, then add a new test:

```rust
#[test]
fn test_symbol_extraction() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    db.index_repo("https://github.com/ylow/SFrameRust/").unwrap();
    let stats = db.parse_symbols().unwrap();

    assert!(stats.blobs_parsed > 0, "Should have parsed some blobs");
    assert!(stats.symbols_extracted > 0, "Should have extracted symbols");

    let conn = db.conn();

    // Verify symbols table populated
    let symbol_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert!(symbol_count > 0, "Should have symbols");

    // Verify symbol_refs table populated
    let ref_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbol_refs", [], |r| r.get(0))
        .unwrap();
    assert!(ref_count > 0, "Should have symbol refs");

    // Verify all parsed blobs are marked
    let unparsed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM blobs WHERE parsed = 0 AND language IN ('rust', 'python', 'javascript', 'typescript', 'tsx', 'go', 'c', 'cpp')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unparsed, 0, "All supported-language blobs should be parsed");

    // Verify we can run the "what calls X" query pattern from DESIGN.md
    let callers: Vec<(String, String)> = conn
        .prepare(
            "SELECT DISTINCT s.name, s.kind
             FROM symbol_refs sr
             JOIN symbols s ON s.id = sr.symbol_id
             WHERE sr.kind = 'call'
             LIMIT 10"
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!callers.is_empty(), "Should find callers via symbol_refs join");

    // Verify parent_id works — find symbols with parents
    let nested_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE parent_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(nested_count > 0, "Should have nested symbols (methods inside impl blocks)");

    // Run parse_symbols again — should be a no-op
    let stats2 = db.parse_symbols().unwrap();
    assert_eq!(stats2.blobs_parsed, 0, "Second run should parse 0 blobs");
}
```

**Step 2: Run integration tests**

Run: `cargo test -p codedb-core --test integration -- --nocapture`
Expected: All tests pass (including existing ones)

If any test fails, investigate and fix. Common issues:
- Tree-sitter queries may not match certain AST patterns in the actual codebase
- The `parse_symbols` function may need adjustments for edge cases

**Step 3: Commit**

```bash
git add codedb-core/tests/integration.rs
git commit -m "test: add integration test for tree-sitter symbol extraction"
```

---

### Task 7: Polish and final verification

**Step 1: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Fix any warnings.

**Step 2: Run all tests**

Run: `cargo test --workspace`
All tests should pass.

**Step 3: Commit and push**

```bash
git add -A
git commit -m "chore: clippy fixes and polish for tree-sitter integration"
git push
```

---

### Task Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1    | Add tree-sitter deps + update schema | — |
| 2    | LanguageConfig struct and registry | 1 |
| 3    | Generic extraction function + unit tests | 2 |
| 4    | parse_symbols() DB integration | 3 |
| 5    | Wire into CLI | 4 |
| 6    | Integration test with SFrameRust | 4 |
| 7    | Polish + final verification | 5, 6 |

Tasks 5 and 6 can be parallelized after Task 4.
Task 3 is the most complex — expect tree-sitter query iteration.
