use anyhow::{Context, Result};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// Configuration for a single language's tree-sitter grammar and queries.
pub(crate) struct LanguageConfig {
    pub language: Language,
    pub def_query: &'static str,
    pub ref_query: &'static str,
}

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
    /// Index into the symbols vec of the parent symbol, if this symbol is nested.
    pub parent_index: Option<usize>,
}

/// A reference (call site) extracted from source code.
#[derive(Debug, Clone)]
pub(crate) struct ExtractedRef {
    pub ref_name: String,
    pub kind: String,
    pub line: usize,
    pub col: usize,
    /// Index into the symbols vec of the containing symbol definition.
    pub containing_symbol_index: Option<usize>,
}

// ---------------------------------------------------------------------------
// Language registry
// ---------------------------------------------------------------------------

const SUPPORTED: &[&str] = &[
    "rust",
    "python",
    "javascript",
    "typescript",
    "tsx",
    "go",
    "c",
    "cpp",
];

/// Returns the list of languages for which we have tree-sitter configs.
pub(crate) fn supported_languages() -> &'static [&'static str] {
    SUPPORTED
}

/// Look up a `LanguageConfig` by the language name string (as returned by
/// `language::detect_language`).
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

// ---------------------------------------------------------------------------
// Per-language configs
// ---------------------------------------------------------------------------

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
            (class_declaration name: (identifier) @name) @def
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
        def_query: "
            (function_declaration name: (identifier) @name) @def
            (class_declaration name: (identifier) @name) @def
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
            (enum_specifier name: (type_identifier) @name) @def
            (class_specifier name: (type_identifier) @name) @def
            (namespace_definition name: (identifier) @name) @def
        ",
        ref_query: "
            (call_expression function: (identifier) @ref_name) @ref
            (call_expression function: (qualified_identifier name: (identifier) @ref_name)) @ref
            (call_expression function: (field_expression field: (field_identifier) @ref_name)) @ref
        ",
    }
}

// ---------------------------------------------------------------------------
// Extraction logic
// ---------------------------------------------------------------------------

/// Map tree-sitter node kind strings to consistent, language-agnostic kind names.
fn normalize_kind(ts_kind: &str) -> String {
    match ts_kind {
        // Rust
        "function_item" => "function".to_string(),
        "struct_item" => "struct".to_string(),
        "enum_item" => "enum".to_string(),
        "trait_item" => "trait".to_string(),
        "impl_item" => "impl".to_string(),
        "const_item" => "const".to_string(),
        "static_item" => "static".to_string(),
        "mod_item" => "module".to_string(),
        // Python / C / C++ (function_definition is shared across grammars)
        "function_definition" => "function".to_string(),
        "class_definition" => "class".to_string(),
        // JS / TS / Go (function_declaration / class_declaration shared)
        "function_declaration" => "function".to_string(),
        "class_declaration" => "class".to_string(),
        "method_definition" => "method".to_string(),
        "interface_declaration" => "interface".to_string(),
        "enum_declaration" => "enum".to_string(),
        "type_alias_declaration" => "type_alias".to_string(),
        // Go
        "method_declaration" => "method".to_string(),
        "type_declaration" | "type_spec" => "type".to_string(),
        // C / C++
        "struct_specifier" => "struct".to_string(),
        "enum_specifier" => "enum".to_string(),
        "class_specifier" => "class".to_string(),
        "namespace_definition" => "namespace".to_string(),
        // Fallback
        other => other.to_string(),
    }
}

/// Extract symbol definitions and references from `source` using the given
/// `LanguageConfig`.
///
/// Returns `None` if parsing fails.
pub(crate) fn extract_symbols(
    source: &str,
    config: &LanguageConfig,
) -> Option<(Vec<ExtractedSymbol>, Vec<ExtractedRef>)> {
    let mut parser = Parser::new();
    parser.set_language(&config.language).ok()?;
    let tree = parser.parse(source, None)?;

    // ---- Phase 1: extract definition symbols ----
    let def_query = Query::new(&config.language, config.def_query).ok()?;
    let name_idx = def_query.capture_index_for_name("name")?;
    let def_idx = def_query.capture_index_for_name("def")?;

    let mut symbols: Vec<ExtractedSymbol> = Vec::new();
    {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&def_query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name_text: Option<&str> = None;
            let mut def_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name_text =
                        Some(&source[cap.node.byte_range()]);
                }
                if cap.index == def_idx {
                    def_node = Some(cap.node);
                }
            }

            if let (Some(name), Some(node)) = (name_text, def_node) {
                let start = node.start_position();
                let end = node.end_position();
                symbols.push(ExtractedSymbol {
                    name: name.to_string(),
                    kind: normalize_kind(node.kind()),
                    line: start.row + 1,   // 1-based
                    col: start.column + 1, // 1-based
                    end_line: end.row + 1,
                    end_col: end.column + 1,
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                    parent_index: None, // filled in below
                });
            }
        }
    }

    // Sort symbols by start_byte so nesting detection works correctly.
    symbols.sort_by_key(|s| (s.start_byte, std::cmp::Reverse(s.end_byte)));

    // ---- Phase 2: determine parent_index via a stack ----
    // After sorting, if symbol B starts inside symbol A's byte range, A is B's
    // nearest enclosing parent.  We use a stack of (index, end_byte).
    {
        let mut stack: Vec<(usize, usize)> = Vec::new(); // (index, end_byte)
        for (i, sym) in symbols.iter_mut().enumerate() {
            // Pop anything that has already ended before this symbol starts.
            while let Some(&(_, parent_end)) = stack.last() {
                if parent_end <= sym.start_byte {
                    stack.pop();
                } else {
                    break;
                }
            }
            if let Some(&(parent_idx, _)) = stack.last() {
                sym.parent_index = Some(parent_idx);
            }
            stack.push((i, sym.end_byte));
        }
    }

    // ---- Phase 3: extract references ----
    let ref_query = Query::new(&config.language, config.ref_query).ok()?;
    let ref_name_idx = ref_query.capture_index_for_name("ref_name")?;
    let ref_idx = ref_query.capture_index_for_name("ref")?;

    let mut refs: Vec<ExtractedRef> = Vec::new();
    {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&ref_query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let mut rname: Option<&str> = None;
            let mut ref_node = None;

            for cap in m.captures {
                if cap.index == ref_name_idx {
                    rname = Some(&source[cap.node.byte_range()]);
                }
                if cap.index == ref_idx {
                    ref_node = Some(cap.node);
                }
            }

            if let (Some(name), Some(node)) = (rname, ref_node) {
                let start = node.start_position();

                // Find innermost containing symbol definition.
                let containing = find_containing_symbol(&symbols, node.start_byte());

                refs.push(ExtractedRef {
                    ref_name: name.to_string(),
                    kind: "call".to_string(),
                    line: start.row + 1,
                    col: start.column + 1,
                    containing_symbol_index: containing,
                });
            }
        }
    }

    Some((symbols, refs))
}

/// Binary-search-ish scan to find the innermost symbol whose byte range
/// contains `byte_offset`.  Since symbols are sorted by (start_byte,
/// reverse end_byte), a simple linear scan from the end works well for
/// typical file sizes.
fn find_containing_symbol(symbols: &[ExtractedSymbol], byte_offset: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, sym) in symbols.iter().enumerate() {
        if sym.start_byte <= byte_offset && byte_offset < sym.end_byte {
            // Prefer the innermost (latest in sorted order that still contains).
            best = Some(i);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Database integration
// ---------------------------------------------------------------------------

pub struct ParseStats {
    pub blobs_parsed: u64,
    pub symbols_extracted: u64,
}

/// Parse symbols for all unparsed blobs that have a supported language.
/// Reads blob content from git repos listed in the repos table.
pub fn parse_symbols(
    conn: &rusqlite::Connection,
    repos_dir: &std::path::Path,
) -> Result<ParseStats> {
    let langs = supported_languages();
    let placeholders: Vec<String> = langs
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let in_clause = placeholders.join(", ");

    let query = format!(
        "SELECT id, content_hash, language FROM blobs WHERE parsed = 0 AND language IN ({in_clause})"
    );
    let mut stmt = conn.prepare(&query)?;
    let params: Vec<&dyn rusqlite::types::ToSql> =
        langs.iter().map(|l| l as &dyn rusqlite::types::ToSql).collect();
    let rows: Vec<(i64, String, String)> = stmt
        .query_map(params.as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;

    if rows.is_empty() {
        return Ok(ParseStats {
            blobs_parsed: 0,
            symbols_extracted: 0,
        });
    }

    // Open all repos for reading blob content
    let mut repos = Vec::new();
    {
        let mut repo_stmt = conn.prepare("SELECT path FROM repos")?;
        let paths: Vec<String> = repo_stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for path_str in paths {
            let full_path = if std::path::Path::new(&path_str).is_absolute() {
                std::path::PathBuf::from(&path_str)
            } else {
                repos_dir.join(&path_str)
            };
            if let Ok(repo) = gix::open(&full_path) {
                repos.push(repo);
            }
        }
    }

    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<ParseStats> {
        let mut total_symbols = 0u64;
        let mut total_blobs = 0u64;

        for (blob_id, content_hash, language) in &rows {
            let config = match get_config(language) {
                Some(c) => c,
                None => continue,
            };

            // Read blob content from git object store
            let oid = gix::ObjectId::from_hex(content_hash.as_bytes())
                .context("Invalid content_hash")?;
            let content = repos.iter().find_map(|repo| {
                repo.find_object(oid)
                    .ok()
                    .and_then(|obj| String::from_utf8(obj.data.clone()).ok())
            });

            let content = match content {
                Some(c) => c,
                None => {
                    // Mark as parsed even if content not found (binary or missing)
                    conn.execute(
                        "UPDATE blobs SET parsed = 1 WHERE id = ?1",
                        rusqlite::params![blob_id],
                    )?;
                    continue;
                }
            };

            if let Some((symbols, refs)) = extract_symbols(&content, &config) {
                let mut symbol_db_ids: Vec<i64> = Vec::with_capacity(symbols.len());
                for sym in &symbols {
                    conn.execute(
                        "INSERT INTO symbols (blob_id, parent_id, name, kind, line, col, end_line, end_col)
                         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            blob_id, sym.name, sym.kind,
                            sym.line, sym.col, sym.end_line, sym.end_col
                        ],
                    )?;
                    symbol_db_ids.push(conn.last_insert_rowid());
                }

                // Update parent_id for nested symbols
                for (i, sym) in symbols.iter().enumerate() {
                    if let Some(parent_idx) = sym.parent_index {
                        let parent_db_id = symbol_db_ids[parent_idx];
                        let sym_db_id = symbol_db_ids[i];
                        conn.execute(
                            "UPDATE symbols SET parent_id = ?1 WHERE id = ?2",
                            rusqlite::params![parent_db_id, sym_db_id],
                        )?;
                    }
                }

                // Insert refs
                for r in &refs {
                    let symbol_id =
                        r.containing_symbol_index.map(|idx| symbol_db_ids[idx]);
                    conn.execute(
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

            conn.execute(
                "UPDATE blobs SET parsed = 1 WHERE id = ?1",
                rusqlite::params![blob_id],
            )?;
        }

        Ok(ParseStats {
            blobs_parsed: total_blobs,
            symbols_extracted: total_symbols,
        })
    })();

    match result {
        Ok(stats) => {
            conn.execute_batch("COMMIT")?;
            Ok(stats)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_function_def() {
        let source = r#"
fn hello() {
    println!("hi");
}

fn world() {
    hello();
}
"#;
        let config = rust_config();
        let (symbols, refs) = extract_symbols(source, &config).expect("extraction failed");

        // Two function symbols
        assert_eq!(symbols.len(), 2, "expected 2 symbols, got: {symbols:#?}");
        assert_eq!(symbols[0].name, "hello");
        assert_eq!(symbols[0].kind, "function");
        assert_eq!(symbols[1].name, "world");
        assert_eq!(symbols[1].kind, "function");

        // hello() call inside world, plus println! macro
        let hello_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "hello").collect();
        assert_eq!(
            hello_refs.len(),
            1,
            "expected 1 hello ref, got: {refs:#?}"
        );
        assert_eq!(
            hello_refs[0].containing_symbol_index,
            Some(1),
            "hello() ref should be inside world"
        );

        // println! macro ref
        let println_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "println").collect();
        assert_eq!(
            println_refs.len(),
            1,
            "expected 1 println ref, got: {refs:#?}"
        );
    }

    #[test]
    fn test_rust_struct_and_impl() {
        let source = r#"
struct Foo {
    x: i32,
}

impl Foo {
    fn bar(&self) {
        baz();
    }
}
"#;
        let config = rust_config();
        let (symbols, refs) = extract_symbols(source, &config).expect("extraction failed");

        // Expect: Foo struct, Foo impl, bar function
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "missing Foo: {names:?}");
        assert!(names.contains(&"bar"), "missing bar: {names:?}");

        let foo_struct = symbols.iter().find(|s| s.name == "Foo" && s.kind == "struct");
        assert!(foo_struct.is_some(), "missing Foo struct");

        let foo_impl = symbols.iter().find(|s| s.name == "Foo" && s.kind == "impl");
        assert!(foo_impl.is_some(), "missing Foo impl");

        let bar_sym = symbols.iter().find(|s| s.name == "bar").unwrap();
        assert_eq!(bar_sym.kind, "function");
        // bar should be nested inside impl Foo
        assert!(bar_sym.parent_index.is_some(), "bar should have a parent");
        let parent = &symbols[bar_sym.parent_index.unwrap()];
        assert_eq!(parent.name, "Foo");
        assert_eq!(parent.kind, "impl");

        // baz() call ref, inside bar
        let baz_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "baz").collect();
        assert_eq!(baz_refs.len(), 1);
        let bar_idx = symbols.iter().position(|s| s.name == "bar").unwrap();
        assert_eq!(baz_refs[0].containing_symbol_index, Some(bar_idx));
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
        let config = python_config();
        let (symbols, refs) = extract_symbols(source, &config).expect("extraction failed");

        // MyClass, method, standalone
        assert_eq!(symbols.len(), 3, "expected 3 symbols, got: {symbols:#?}");

        let myclass = symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(myclass.kind, "class");

        let method = symbols.iter().find(|s| s.name == "method").unwrap();
        assert_eq!(method.kind, "function");
        assert!(
            method.parent_index.is_some(),
            "method should be nested in MyClass"
        );
        let parent = &symbols[method.parent_index.unwrap()];
        assert_eq!(parent.name, "MyClass");

        let standalone = symbols.iter().find(|s| s.name == "standalone").unwrap();
        assert!(standalone.parent_index.is_none());

        // other_func() reference
        let other_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "other_func").collect();
        assert_eq!(other_refs.len(), 1);
    }

    #[test]
    fn test_go_extraction() {
        let source = r#"
package main

func hello() {
    world()
}

func world() {
}
"#;
        let config = go_config();
        let (symbols, refs) = extract_symbols(source, &config).expect("extraction failed");

        assert_eq!(symbols.len(), 2, "expected 2 symbols, got: {symbols:#?}");
        assert_eq!(symbols[0].name, "hello");
        assert_eq!(symbols[1].name, "world");

        let world_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "world").collect();
        assert_eq!(world_refs.len(), 1, "expected world() call ref");
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
        let config = c_config();
        let (symbols, refs) = extract_symbols(source, &config).expect("extraction failed");

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Point"), "missing Point: {names:?}");
        assert!(names.contains(&"add"), "missing add: {names:?}");
        assert!(names.contains(&"main"), "missing main: {names:?}");

        let point = symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.kind, "struct");

        // add() call reference inside main
        let add_refs: Vec<_> = refs.iter().filter(|r| r.ref_name == "add").collect();
        assert_eq!(add_refs.len(), 1);
        let main_idx = symbols.iter().position(|s| s.name == "main").unwrap();
        assert_eq!(add_refs[0].containing_symbol_index, Some(main_idx));
    }

    #[test]
    fn test_unsupported_language() {
        assert!(get_config("fortran").is_none());
        assert!(get_config("haskell").is_none());
    }

    #[test]
    fn test_supported_languages() {
        let langs = supported_languages();
        assert_eq!(langs.len(), 8);
        for expected in &[
            "rust",
            "python",
            "javascript",
            "typescript",
            "tsx",
            "go",
            "c",
            "cpp",
        ] {
            assert!(
                langs.contains(expected),
                "{expected} not in supported_languages"
            );
        }
    }
}
