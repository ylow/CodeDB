# Sourcegraph Query Translator Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a Sourcegraph-compatible query language frontend that translates queries like `lang:rust type:symbol process_data` into SQL, executes them, and returns formatted results.

**Architecture:** New `query.rs` module in codedb-core with a tokenizer, parser, and per-search-type SQL generators. The CLI `search` command is upgraded to use it. All filter values are bound via SQL parameters (no string interpolation).

**Tech Stack:** Rust, rusqlite (parameter binding), existing Tantivy vtabs. No new dependencies.

---

### Task 1: Data Types and Tokenizer

**Files:**
- Create: `codedb-core/src/query.rs`
- Modify: `codedb-core/src/lib.rs`

**Context:** This module implements the Sourcegraph query language frontend. It needs to be `pub` since the CLI will use `TranslatedQuery` and `SearchType`. Read the design doc at `docs/plans/2026-03-05-sourcegraph-query-design.md` for full context.

**Step 1: Create `query.rs` with data types and tokenizer**

Add to `codedb-core/src/query.rs`:

```rust
use anyhow::{bail, Result};

/// Search type determined by the `type:` filter.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchType {
    Code,
    Diff,
    Commit,
    Symbol,
}

/// Output selector determined by the `select:` filter.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectType {
    Repo,
    File,
    Symbol,
    SymbolKind(String),
}

/// Parsed filters from the query string.
#[derive(Debug, Clone, Default)]
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

/// Result of parsing a Sourcegraph-style query string.
#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub search_pattern: String,
    pub search_type: SearchType,
    pub filters: Filters,
}

/// SQL query ready for execution, with bound parameters.
#[derive(Debug, Clone)]
pub struct TranslatedQuery {
    pub sql: String,
    pub params: Vec<String>,
    pub search_type: SearchType,
}

/// Split input into tokens, respecting quoted strings.
///
/// - Whitespace separates tokens
/// - `"foo bar"` is a single token (quotes preserved in output)
/// - Everything else is split on whitespace
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '"' => {
                // Flush any accumulated non-quoted text
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                chars.next(); // consume opening quote
                let mut quoted = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    quoted.push(c);
                }
                tokens.push(format!("\"{quoted}\""));
            }
            ' ' | '\t' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                chars.next();
            }
            _ => {
                current.push(ch);
                chars.next();
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}
```

**Step 2: Wire up module in lib.rs**

Add to `codedb-core/src/lib.rs`:

```rust
pub mod query;
```

And add to the public re-exports:

```rust
pub use query::{ParsedQuery, TranslatedQuery, SearchType};
```

**Step 3: Write tokenizer tests**

Add to the bottom of `codedb-core/src/query.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple() {
        assert_eq!(tokenize("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_tokenize_quoted() {
        assert_eq!(
            tokenize("lang:rust \"foo bar\" baz"),
            vec!["lang:rust", "\"foo bar\"", "baz"]
        );
    }

    #[test]
    fn test_tokenize_filter_with_value() {
        assert_eq!(
            tokenize("repo:SFrame file:*.rs fn"),
            vec!["repo:SFrame", "file:*.rs", "fn"]
        );
    }

    #[test]
    fn test_tokenize_negation() {
        assert_eq!(
            tokenize("-file:test foo"),
            vec!["-file:test", "foo"]
        );
    }

    #[test]
    fn test_tokenize_empty() {
        assert_eq!(tokenize(""), Vec::<String>::new());
    }

    #[test]
    fn test_tokenize_extra_whitespace() {
        assert_eq!(tokenize("  foo   bar  "), vec!["foo", "bar"]);
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p codedb-core query::tests -- --nocapture`

Expected: All 6 tokenizer tests pass.

**Step 5: Commit**

```bash
git add codedb-core/src/query.rs codedb-core/src/lib.rs
git commit -m "feat(query): add data types and tokenizer for Sourcegraph query language"
```

---

### Task 2: Query Parser

**Files:**
- Modify: `codedb-core/src/query.rs`

**Context:** The parser takes tokenized input and produces a `ParsedQuery`. Each token is either a `key:value` filter (or `-key:value` for negation) or a search term. Unknown filter keys produce errors. Read Task 1 for the data types.

**Step 1: Add parse_query function and helpers**

Add to `codedb-core/src/query.rs` (after the tokenize function, before `#[cfg(test)]`):

```rust
fn parse_select(value: &str) -> Result<SelectType> {
    match value {
        "repo" => Ok(SelectType::Repo),
        "file" => Ok(SelectType::File),
        "symbol" => Ok(SelectType::Symbol),
        _ if value.starts_with("symbol.") => {
            let kind = &value["symbol.".len()..];
            Ok(SelectType::SymbolKind(kind.to_string()))
        }
        _ => bail!(
            "Unknown select type '{value}'. Valid: repo, file, symbol, symbol.<kind>"
        ),
    }
}

/// Parse a Sourcegraph-style query string into a structured representation.
///
/// Supports filters: `repo:`, `file:`, `-file:`, `lang:`, `type:`, `rev:`,
/// `count:`, `case:`, `author:`, `before:`, `after:`, `message:`, `select:`.
///
/// Everything that isn't a filter becomes the search pattern.
pub fn parse_query(input: &str) -> Result<ParsedQuery> {
    let tokens = tokenize(input);
    let mut filters = Filters::default();
    let mut search_type = SearchType::Code;
    let mut search_terms: Vec<String> = Vec::new();

    for token in &tokens {
        // Check for negated filter (-key:value)
        let (negated, rest) = if let Some(r) = token.strip_prefix('-') {
            (true, r)
        } else {
            (false, token.as_str())
        };

        if let Some((key, value)) = rest.split_once(':') {
            // Validate that key looks like a known filter name (not a search term with a colon)
            match (negated, key) {
                (false, "repo") => filters.repo = Some(value.to_string()),
                (false, "file") => filters.file = Some(value.to_string()),
                (true, "file") => filters.neg_file = Some(value.to_string()),
                (false, "lang" | "language" | "l") => {
                    filters.lang = Some(value.to_string());
                }
                (false, "type") => {
                    search_type = match value {
                        "diff" => SearchType::Diff,
                        "commit" => SearchType::Commit,
                        "symbol" => SearchType::Symbol,
                        _ => bail!(
                            "Unknown search type '{value}'. Valid types: symbol, diff, commit"
                        ),
                    };
                }
                (false, "rev" | "revision") => {
                    filters.rev = Some(value.to_string());
                }
                (false, "count") => {
                    filters.count = Some(value.parse::<u32>().map_err(|_| {
                        anyhow::anyhow!(
                            "count: must be a positive integer, got '{value}'"
                        )
                    })?);
                }
                (false, "case") => {
                    filters.case_sensitive = value == "yes";
                }
                (false, "author") => {
                    filters.author = Some(value.to_string());
                }
                (false, "before") => {
                    filters.before = Some(value.to_string());
                }
                (false, "after") => {
                    filters.after = Some(value.to_string());
                }
                (false, "message") => {
                    filters.message = Some(value.to_string());
                }
                (false, "select") => {
                    filters.select = Some(parse_select(value)?);
                }
                (true, other) => bail!("Negation not supported for '{other}:'"),
                (false, _) => {
                    // Not a known filter — treat as search term
                    // (handles things like URLs with colons in search patterns)
                    search_terms.push(token.clone());
                }
            }
        } else {
            // Not a filter — it's a search term
            let term = token.trim_matches('"');
            search_terms.push(term.to_string());
        }
    }

    let search_pattern = search_terms.join(" ");

    Ok(ParsedQuery {
        search_pattern,
        search_type,
        filters,
    })
}
```

**Step 2: Write parser tests**

Add to the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn test_parse_bare_search() {
        let q = parse_query("foo bar").unwrap();
        assert_eq!(q.search_pattern, "foo bar");
        assert_eq!(q.search_type, SearchType::Code);
        assert!(q.filters.repo.is_none());
    }

    #[test]
    fn test_parse_with_filters() {
        let q = parse_query("lang:rust file:*.rs process_data").unwrap();
        assert_eq!(q.search_pattern, "process_data");
        assert_eq!(q.filters.lang.as_deref(), Some("rust"));
        assert_eq!(q.filters.file.as_deref(), Some("*.rs"));
        assert_eq!(q.search_type, SearchType::Code);
    }

    #[test]
    fn test_parse_type_symbol() {
        let q = parse_query("type:symbol lang:rust SFrame").unwrap();
        assert_eq!(q.search_type, SearchType::Symbol);
        assert_eq!(q.search_pattern, "SFrame");
        assert_eq!(q.filters.lang.as_deref(), Some("rust"));
    }

    #[test]
    fn test_parse_type_diff() {
        let q = parse_query("type:diff author:ylow streaming").unwrap();
        assert_eq!(q.search_type, SearchType::Diff);
        assert_eq!(q.search_pattern, "streaming");
        assert_eq!(q.filters.author.as_deref(), Some("ylow"));
    }

    #[test]
    fn test_parse_type_commit() {
        let q =
            parse_query("type:commit before:2026-01-01 after:2025-06-01 refactor")
                .unwrap();
        assert_eq!(q.search_type, SearchType::Commit);
        assert_eq!(q.search_pattern, "refactor");
        assert_eq!(q.filters.before.as_deref(), Some("2026-01-01"));
        assert_eq!(q.filters.after.as_deref(), Some("2025-06-01"));
    }

    #[test]
    fn test_parse_negated_file() {
        let q = parse_query("-file:test foo").unwrap();
        assert_eq!(q.filters.neg_file.as_deref(), Some("test"));
        assert_eq!(q.search_pattern, "foo");
    }

    #[test]
    fn test_parse_count() {
        let q = parse_query("count:50 foo").unwrap();
        assert_eq!(q.filters.count, Some(50));
    }

    #[test]
    fn test_parse_select_symbol_kind() {
        let q =
            parse_query("type:symbol select:symbol.function foo").unwrap();
        assert_eq!(
            q.filters.select,
            Some(SelectType::SymbolKind("function".to_string()))
        );
    }

    #[test]
    fn test_parse_rev() {
        let q = parse_query("rev:develop foo").unwrap();
        assert_eq!(q.filters.rev.as_deref(), Some("develop"));
    }

    #[test]
    fn test_parse_quoted_phrase() {
        let q = parse_query("lang:rust \"foo bar\"").unwrap();
        assert_eq!(q.search_pattern, "foo bar");
    }

    #[test]
    fn test_parse_unknown_type_error() {
        let err = parse_query("type:bogus foo").unwrap_err();
        assert!(
            err.to_string().contains("Unknown search type"),
            "got: {err}"
        );
    }

    #[test]
    fn test_parse_invalid_count_error() {
        let err = parse_query("count:abc foo").unwrap_err();
        assert!(
            err.to_string().contains("positive integer"),
            "got: {err}"
        );
    }

    #[test]
    fn test_parse_negation_unsupported() {
        let err = parse_query("-repo:foo bar").unwrap_err();
        assert!(
            err.to_string().contains("Negation not supported"),
            "got: {err}"
        );
    }

    #[test]
    fn test_parse_unknown_filter_is_search_term() {
        // Unknown key:value patterns are treated as search terms
        let q = parse_query("http://example.com foo").unwrap();
        assert_eq!(q.search_pattern, "http://example.com foo");
    }

    #[test]
    fn test_parse_lang_alias() {
        let q = parse_query("l:python foo").unwrap();
        assert_eq!(q.filters.lang.as_deref(), Some("python"));
    }
```

**Step 3: Run tests**

Run: `cargo test -p codedb-core query::tests -- --nocapture`

Expected: All tests pass (6 tokenizer + 14 parser = 20 tests).

**Step 4: Commit**

```bash
git add codedb-core/src/query.rs
git commit -m "feat(query): add Sourcegraph query parser with filter support"
```

---

### Task 3: SQL Generator — Code Search

**Files:**
- Modify: `codedb-core/src/query.rs`

**Context:** The SQL generator takes a `ParsedQuery` and produces a `TranslatedQuery` with parameterized SQL. This task covers code search (default type). The `translate()` function dispatches to type-specific generators. Helper functions handle the substring/GLOB pattern matching logic.

**Step 1: Add SQL generation functions**

Add to `codedb-core/src/query.rs` (after `parse_query`, before `#[cfg(test)]`):

```rust
/// Helper: tracks SQL parameters and returns `?N` placeholders.
struct ParamCollector {
    params: Vec<String>,
}

impl ParamCollector {
    fn new() -> Self {
        Self { params: Vec::new() }
    }

    /// Add a parameter and return its `?N` placeholder string.
    fn add(&mut self, value: String) -> String {
        self.params.push(value);
        format!("?{}", self.params.len())
    }
}

/// If pattern contains `*` or `?`, use GLOB. Otherwise use LIKE substring match.
fn pattern_match_clause(column: &str, pattern: &str, p: &mut ParamCollector) -> String {
    if pattern.contains('*') || pattern.contains('?') {
        let placeholder = p.add(pattern.to_string());
        format!("{column} GLOB {placeholder}")
    } else {
        let placeholder = p.add(format!("%{pattern}%"));
        format!("{column} LIKE {placeholder}")
    }
}

/// Translate a parsed query into SQL.
pub fn translate(query: &ParsedQuery) -> Result<TranslatedQuery> {
    match query.search_type {
        SearchType::Code => translate_code(query),
        SearchType::Diff => translate_diff(query),
        SearchType::Commit => translate_commit(query),
        SearchType::Symbol => translate_symbol(query),
    }
}

fn translate_code(query: &ParsedQuery) -> Result<TranslatedQuery> {
    if query.search_pattern.is_empty() {
        bail!("Code search requires a search pattern");
    }

    let mut p = ParamCollector::new();
    let search_param = p.add(query.search_pattern.clone());

    let mut joins = vec![
        "JOIN blobs b ON b.id = cs.blob_id".to_string(),
        "JOIN file_revs fr ON fr.blob_id = b.id".to_string(),
        "JOIN refs r ON r.commit_id = fr.commit_id".to_string(),
    ];
    let mut conditions = Vec::new();

    // repo: filter
    if let Some(ref repo) = query.filters.repo {
        joins.push("JOIN repos rp ON rp.id = r.repo_id".to_string());
        let clause = pattern_match_clause("rp.name", repo, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    // file: filter
    if let Some(ref file) = query.filters.file {
        let clause = pattern_match_clause("fr.path", file, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    // -file: filter
    if let Some(ref neg_file) = query.filters.neg_file {
        let clause = pattern_match_clause("fr.path", neg_file, &mut p);
        conditions.push(format!("AND NOT ({clause})"));
    }

    // lang: filter
    if let Some(ref lang) = query.filters.lang {
        let placeholder = p.add(lang.clone());
        conditions.push(format!("AND b.language = {placeholder}"));
    }

    // rev: filter (defaults to refs/heads/main)
    let rev = query.filters.rev.clone().unwrap_or_else(|| "main".to_string());
    let rev_ref = if rev.starts_with("refs/") {
        rev
    } else {
        format!("refs/heads/{rev}")
    };
    let rev_placeholder = p.add(rev_ref);
    conditions.push(format!("AND r.name = {rev_placeholder}"));

    let limit = query.filters.count.unwrap_or(20);

    // select: modifier changes output
    let (select_clause, group_by, order_by) = match &query.filters.select {
        Some(SelectType::Repo) => {
            // Ensure repos join exists
            if query.filters.repo.is_none() {
                joins.push("JOIN repos rp ON rp.id = r.repo_id".to_string());
            }
            (
                "SELECT DISTINCT rp.name".to_string(),
                String::new(),
                "ORDER BY rp.name".to_string(),
            )
        }
        Some(SelectType::File) => (
            "SELECT DISTINCT fr.path".to_string(),
            String::new(),
            "ORDER BY fr.path".to_string(),
        ),
        _ => (
            "SELECT fr.path, cs.score, cs.snippet".to_string(),
            "GROUP BY fr.path".to_string(),
            "ORDER BY cs.score DESC".to_string(),
        ),
    };

    let joins_str = joins.join("\n");
    let conditions_str = if conditions.is_empty() {
        String::new()
    } else {
        format!("\n  {}", conditions.join("\n  "))
    };

    let sql = format!(
        "{select_clause}\n\
         FROM code_search({search_param}) cs\n\
         {joins_str}\n\
         WHERE 1=1{conditions_str}\n\
         {group_by}\n\
         {order_by}\n\
         LIMIT {limit}"
    );

    // Clean up blank lines from empty group_by
    let sql = sql.lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(TranslatedQuery {
        sql,
        params: p.params,
        search_type: SearchType::Code,
    })
}
```

Also add temporary stubs for the other types so the code compiles:

```rust
fn translate_diff(query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Task 4")
}

fn translate_commit(query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Task 4")
}

fn translate_symbol(query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Task 4")
}
```

**Step 2: Write SQL generation tests for code search**

Add to the `tests` module:

```rust
    #[test]
    fn test_translate_code_basic() {
        let q = parse_query("process_data").unwrap();
        let t = translate(&q).unwrap();
        assert_eq!(t.search_type, SearchType::Code);
        assert!(t.sql.contains("code_search(?1)"));
        assert!(t.sql.contains("LIMIT 20"));
        assert_eq!(t.params[0], "process_data");
        // Default rev filter
        assert!(t.sql.contains("r.name ="));
        assert!(t.params.iter().any(|p| p.contains("refs/heads/main")));
    }

    #[test]
    fn test_translate_code_with_filters() {
        let q = parse_query("lang:rust file:*.rs count:10 foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("b.language ="));
        assert!(t.sql.contains("fr.path GLOB"));
        assert!(t.sql.contains("LIMIT 10"));
        assert_eq!(t.params[0], "foo"); // search pattern is always first param
    }

    #[test]
    fn test_translate_code_neg_file() {
        let q = parse_query("-file:test foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("NOT"));
        assert!(t.sql.contains("fr.path LIKE"));
    }

    #[test]
    fn test_translate_code_repo_filter() {
        let q = parse_query("repo:SFrame foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("repos rp"));
        assert!(t.sql.contains("rp.name LIKE"));
    }

    #[test]
    fn test_translate_code_select_file() {
        let q = parse_query("select:file foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("SELECT DISTINCT fr.path"));
        assert!(!t.sql.contains("cs.score"));
    }

    #[test]
    fn test_translate_code_custom_rev() {
        let q = parse_query("rev:develop foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.params.iter().any(|p| p == "refs/heads/develop"));
    }

    #[test]
    fn test_translate_code_empty_pattern_error() {
        let q = parse_query("lang:rust").unwrap();
        let err = translate(&q).unwrap_err();
        assert!(
            err.to_string().contains("requires a search pattern"),
            "got: {err}"
        );
    }

    #[test]
    fn test_translate_code_substring_match() {
        let q = parse_query("file:csv foo").unwrap();
        let t = translate(&q).unwrap();
        // No wildcards, so should use LIKE with %
        assert!(t.sql.contains("LIKE"));
        assert!(t.params.iter().any(|p| p == "%csv%"));
    }

    #[test]
    fn test_translate_code_glob_match() {
        let q = parse_query("file:*.rs foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("GLOB"));
        assert!(t.params.iter().any(|p| p == "*.rs"));
    }
```

**Step 3: Run tests**

Run: `cargo test -p codedb-core query::tests -- --nocapture`

Expected: All tests pass.

**Step 4: Commit**

```bash
git add codedb-core/src/query.rs
git commit -m "feat(query): add SQL generator for code search"
```

---

### Task 4: SQL Generator — Diff, Commit, and Symbol Search

**Files:**
- Modify: `codedb-core/src/query.rs`

**Context:** Replace the three `todo!()` stubs with real implementations. These follow the same pattern as code search but target different tables/vtabs. Read the design doc's SQL templates for each type.

**Step 1: Implement translate_diff**

Replace the `translate_diff` stub:

```rust
fn translate_diff(query: &ParsedQuery) -> Result<TranslatedQuery> {
    if query.search_pattern.is_empty() {
        bail!("Diff search requires a search pattern");
    }

    let mut p = ParamCollector::new();
    let search_param = p.add(query.search_pattern.clone());

    let mut joins = vec![
        "JOIN diffs d ON d.id = ds.diff_id".to_string(),
        "JOIN commits c ON c.id = d.commit_id".to_string(),
    ];
    let mut conditions = Vec::new();

    if let Some(ref repo) = query.filters.repo {
        joins.push("JOIN repos rp ON rp.id = c.repo_id".to_string());
        let clause = pattern_match_clause("rp.name", repo, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref file) = query.filters.file {
        let clause = pattern_match_clause("d.path", file, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref neg_file) = query.filters.neg_file {
        let clause = pattern_match_clause("d.path", neg_file, &mut p);
        conditions.push(format!("AND NOT ({clause})"));
    }

    if let Some(ref author) = query.filters.author {
        let clause = pattern_match_clause("c.author", author, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref before) = query.filters.before {
        let placeholder = p.add(before.clone());
        conditions.push(format!(
            "AND c.timestamp < CAST(strftime('%s', {placeholder}) AS INTEGER)"
        ));
    }

    if let Some(ref after) = query.filters.after {
        let placeholder = p.add(after.clone());
        conditions.push(format!(
            "AND c.timestamp > CAST(strftime('%s', {placeholder}) AS INTEGER)"
        ));
    }

    let limit = query.filters.count.unwrap_or(20);

    let conditions_str = if conditions.is_empty() {
        String::new()
    } else {
        format!("\n  {}", conditions.join("\n  "))
    };

    let joins_str = joins.join("\n");

    let select_clause = match &query.filters.select {
        Some(SelectType::File) => "SELECT DISTINCT d.path",
        _ => "SELECT substr(c.hash, 1, 10) AS hash,\n       \
              substr(c.message, 1, 80) AS message,\n       \
              d.path, round(ds.score, 2) AS score",
    };

    let order_by = match &query.filters.select {
        Some(SelectType::File) => "ORDER BY d.path",
        _ => "ORDER BY ds.score DESC",
    };

    let sql = format!(
        "{select_clause}\n\
         FROM diff_search({search_param}) ds\n\
         {joins_str}\n\
         WHERE 1=1{conditions_str}\n\
         {order_by}\n\
         LIMIT {limit}"
    );

    Ok(TranslatedQuery {
        sql,
        params: p.params,
        search_type: SearchType::Diff,
    })
}
```

**Step 2: Implement translate_commit**

Replace the `translate_commit` stub:

```rust
fn translate_commit(query: &ParsedQuery) -> Result<TranslatedQuery> {
    let mut p = ParamCollector::new();
    let mut conditions = Vec::new();

    // For commit search, the pattern matches the commit message
    if !query.search_pattern.is_empty() {
        let clause = pattern_match_clause("c.message", &query.search_pattern, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    // message: filter (explicit, in addition to or instead of bare pattern)
    if let Some(ref message) = query.filters.message {
        let clause = pattern_match_clause("c.message", message, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    let mut joins = Vec::new();

    if let Some(ref repo) = query.filters.repo {
        joins.push("JOIN repos rp ON rp.id = c.repo_id".to_string());
        let clause = pattern_match_clause("rp.name", repo, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref author) = query.filters.author {
        let clause = pattern_match_clause("c.author", author, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref before) = query.filters.before {
        let placeholder = p.add(before.clone());
        conditions.push(format!(
            "AND c.timestamp < CAST(strftime('%s', {placeholder}) AS INTEGER)"
        ));
    }

    if let Some(ref after) = query.filters.after {
        let placeholder = p.add(after.clone());
        conditions.push(format!(
            "AND c.timestamp > CAST(strftime('%s', {placeholder}) AS INTEGER)"
        ));
    }

    // Require at least one filter so we don't scan all commits with no conditions
    if conditions.is_empty() {
        bail!(
            "Commit search requires a search pattern or at least one filter \
             (author:, before:, after:, message:)"
        );
    }

    let limit = query.filters.count.unwrap_or(20);

    let conditions_str = format!("\n  {}", conditions.join("\n  "));

    let joins_str = if joins.is_empty() {
        String::new()
    } else {
        format!("\n{}", joins.join("\n"))
    };

    let sql = format!(
        "SELECT substr(c.hash, 1, 10) AS hash, c.author,\n       \
         substr(c.message, 1, 80) AS message\n\
         FROM commits c{joins_str}\n\
         WHERE 1=1{conditions_str}\n\
         ORDER BY c.timestamp DESC\n\
         LIMIT {limit}"
    );

    Ok(TranslatedQuery {
        sql,
        params: p.params,
        search_type: SearchType::Commit,
    })
}
```

**Step 3: Implement translate_symbol**

Replace the `translate_symbol` stub:

```rust
fn translate_symbol(query: &ParsedQuery) -> Result<TranslatedQuery> {
    let mut p = ParamCollector::new();
    let mut joins = vec![
        "JOIN blobs b ON b.id = s.blob_id".to_string(),
        "JOIN file_revs fr ON fr.blob_id = b.id".to_string(),
        "JOIN refs r ON r.commit_id = fr.commit_id".to_string(),
    ];
    let mut conditions = Vec::new();

    // Search pattern matches symbol name
    if !query.search_pattern.is_empty() {
        let clause =
            pattern_match_clause("s.name", &query.search_pattern, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref repo) = query.filters.repo {
        joins.push("JOIN repos rp ON rp.id = r.repo_id".to_string());
        let clause = pattern_match_clause("rp.name", repo, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref file) = query.filters.file {
        let clause = pattern_match_clause("fr.path", file, &mut p);
        conditions.push(format!("AND {clause}"));
    }

    if let Some(ref neg_file) = query.filters.neg_file {
        let clause = pattern_match_clause("fr.path", neg_file, &mut p);
        conditions.push(format!("AND NOT ({clause})"));
    }

    if let Some(ref lang) = query.filters.lang {
        let placeholder = p.add(lang.clone());
        conditions.push(format!("AND b.language = {placeholder}"));
    }

    // select:symbol.function → filter by kind
    if let Some(SelectType::SymbolKind(ref kind)) = query.filters.select {
        let placeholder = p.add(kind.clone());
        conditions.push(format!("AND s.kind = {placeholder}"));
    }

    // rev: filter
    let rev = query
        .filters
        .rev
        .clone()
        .unwrap_or_else(|| "main".to_string());
    let rev_ref = if rev.starts_with("refs/") {
        rev
    } else {
        format!("refs/heads/{rev}")
    };
    let rev_placeholder = p.add(rev_ref);
    conditions.push(format!("AND r.name = {rev_placeholder}"));

    // Require at least a name pattern or kind filter
    if query.search_pattern.is_empty()
        && !matches!(query.filters.select, Some(SelectType::SymbolKind(_)))
        && query.filters.lang.is_none()
        && query.filters.file.is_none()
    {
        bail!(
            "Symbol search requires a search pattern or filter \
             (lang:, file:, select:symbol.<kind>)"
        );
    }

    let limit = query.filters.count.unwrap_or(20);

    let conditions_str = if conditions.is_empty() {
        String::new()
    } else {
        format!("\n  {}", conditions.join("\n  "))
    };

    let joins_str = joins.join("\n");

    let sql = format!(
        "SELECT fr.path, s.name, s.kind, s.line\n\
         FROM symbols s\n\
         {joins_str}\n\
         WHERE 1=1{conditions_str}\n\
         ORDER BY fr.path, s.line\n\
         LIMIT {limit}"
    );

    Ok(TranslatedQuery {
        sql,
        params: p.params,
        search_type: SearchType::Symbol,
    })
}
```

**Step 4: Write tests for diff, commit, symbol generators**

Add to the `tests` module:

```rust
    // --- Diff search tests ---

    #[test]
    fn test_translate_diff_basic() {
        let q = parse_query("type:diff streaming").unwrap();
        let t = translate(&q).unwrap();
        assert_eq!(t.search_type, SearchType::Diff);
        assert!(t.sql.contains("diff_search(?1)"));
        assert_eq!(t.params[0], "streaming");
    }

    #[test]
    fn test_translate_diff_with_author() {
        let q = parse_query("type:diff author:ylow streaming").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("c.author"));
        assert!(t.params.iter().any(|p| p.contains("ylow")));
    }

    #[test]
    fn test_translate_diff_with_dates() {
        let q = parse_query(
            "type:diff before:2026-01-01 after:2025-06-01 streaming",
        )
        .unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("c.timestamp <"));
        assert!(t.sql.contains("c.timestamp >"));
        assert!(t.sql.contains("strftime"));
    }

    #[test]
    fn test_translate_diff_empty_pattern_error() {
        let q = parse_query("type:diff").unwrap();
        let err = translate(&q).unwrap_err();
        assert!(
            err.to_string().contains("requires a search pattern"),
            "got: {err}"
        );
    }

    // --- Commit search tests ---

    #[test]
    fn test_translate_commit_basic() {
        let q = parse_query("type:commit refactor").unwrap();
        let t = translate(&q).unwrap();
        assert_eq!(t.search_type, SearchType::Commit);
        assert!(t.sql.contains("commits c"));
        assert!(t.sql.contains("c.message LIKE"));
    }

    #[test]
    fn test_translate_commit_author_only() {
        let q = parse_query("type:commit author:ylow").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("c.author"));
    }

    #[test]
    fn test_translate_commit_no_filters_error() {
        let q = parse_query("type:commit").unwrap();
        let err = translate(&q).unwrap_err();
        assert!(
            err.to_string().contains("requires a search pattern"),
            "got: {err}"
        );
    }

    // --- Symbol search tests ---

    #[test]
    fn test_translate_symbol_basic() {
        let q = parse_query("type:symbol process_data").unwrap();
        let t = translate(&q).unwrap();
        assert_eq!(t.search_type, SearchType::Symbol);
        assert!(t.sql.contains("symbols s"));
        assert!(t.sql.contains("s.name LIKE"));
    }

    #[test]
    fn test_translate_symbol_with_lang() {
        let q = parse_query("type:symbol lang:rust SFrame").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("b.language ="));
    }

    #[test]
    fn test_translate_symbol_kind_filter() {
        let q = parse_query(
            "type:symbol select:symbol.function lang:rust foo",
        )
        .unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("s.kind ="));
        assert!(t.params.iter().any(|p| p == "function"));
    }

    #[test]
    fn test_translate_symbol_no_filters_error() {
        let q = parse_query("type:symbol").unwrap();
        let err = translate(&q).unwrap_err();
        assert!(
            err.to_string().contains("requires a search pattern"),
            "got: {err}"
        );
    }
```

**Step 5: Run tests**

Run: `cargo test -p codedb-core query::tests -- --nocapture`

Expected: All tests pass.

**Step 6: Commit**

```bash
git add codedb-core/src/query.rs
git commit -m "feat(query): add SQL generators for diff, commit, and symbol search"
```

---

### Task 5: CodeDB Public API

**Files:**
- Modify: `codedb-core/src/codedb.rs`
- Modify: `codedb-core/src/lib.rs`

**Context:** Add `search()` and `translate_query()` methods to `CodeDB`. The `search()` method parses a Sourcegraph query, translates to SQL, executes it, and returns structured results. The `translate_query()` method just returns the SQL without executing (for `--sql`).

**Step 1: Add search result types and methods**

Add to `codedb-core/src/query.rs` (after `TranslatedQuery`, before `fn tokenize`):

```rust
/// A single row of search results.
#[derive(Debug, Clone)]
pub struct SearchResultRow {
    pub columns: Vec<(String, String)>,
}

/// Results from executing a search query.
#[derive(Debug, Clone)]
pub struct SearchResults {
    pub search_type: SearchType,
    pub rows: Vec<SearchResultRow>,
}
```

Add to `codedb-core/src/codedb.rs` — first add the import at the top:

```rust
use crate::query::{self, SearchResults, SearchResultRow, TranslatedQuery};
```

Then add these methods to the `impl CodeDB` block:

```rust
    /// Parse and translate a Sourcegraph-style query to SQL without executing.
    pub fn translate_query(&self, input: &str) -> Result<TranslatedQuery> {
        let parsed = query::parse_query(input)?;
        query::translate(&parsed)
    }

    /// Parse, translate, and execute a Sourcegraph-style query.
    pub fn search(&self, input: &str) -> Result<SearchResults> {
        let translated = self.translate_query(input)?;
        let search_type = translated.search_type.clone();

        let mut stmt = self.conn.prepare(&translated.sql)?;

        // Bind parameters
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = translated
            .params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let col_count = stmt.column_count();
        let col_names: Vec<String> = (0..col_count)
            .map(|i| stmt.column_name(i).unwrap().to_string())
            .collect();

        let mut rows = Vec::new();
        let mut result_rows = stmt.query(param_refs.as_slice())?;
        while let Some(row) = result_rows.next()? {
            let columns: Vec<(String, String)> = (0..col_count)
                .map(|i| {
                    let name = col_names[i].clone();
                    let val = row
                        .get::<_, rusqlite::types::Value>(i)
                        .map(|v| match v {
                            rusqlite::types::Value::Null => "NULL".to_string(),
                            rusqlite::types::Value::Integer(n) => n.to_string(),
                            rusqlite::types::Value::Real(f) => format!("{f:.2}"),
                            rusqlite::types::Value::Text(s) => s,
                            rusqlite::types::Value::Blob(_) => "<blob>".to_string(),
                        })
                        .unwrap_or_else(|_| "NULL".to_string());
                    (name, val)
                })
                .collect();
            rows.push(SearchResultRow { columns });
        }

        Ok(SearchResults { search_type, rows })
    }
```

**Step 2: Update lib.rs re-exports**

Update `codedb-core/src/lib.rs` to export the new types:

```rust
pub use query::{ParsedQuery, SearchResults, SearchResultRow, SearchType, TranslatedQuery};
```

**Step 3: Write unit test**

Add to `codedb-core/src/codedb.rs` tests module:

```rust
    #[test]
    fn test_translate_query() {
        let tmp = TempDir::new().unwrap();
        let db = CodeDB::open(tmp.path()).unwrap();
        let t = db.translate_query("lang:rust foo").unwrap();
        assert!(t.sql.contains("code_search"));
        assert!(t.sql.contains("b.language"));
    }
```

**Step 4: Run tests**

Run: `cargo test -p codedb-core -- --nocapture`

Expected: All tests pass.

**Step 5: Commit**

```bash
git add codedb-core/src/query.rs codedb-core/src/codedb.rs codedb-core/src/lib.rs
git commit -m "feat(query): add CodeDB search() and translate_query() public API"
```

---

### Task 6: CLI Integration

**Files:**
- Modify: `codedb-cli/src/main.rs`

**Context:** Upgrade the `search` command to use the Sourcegraph query translator. Add a `--sql` flag that prints the generated SQL. Format output based on search type. The existing `search` behavior (bare terms = full-text code search) is preserved.

**Step 1: Update the CLI**

Replace the contents of `codedb-cli/src/main.rs`:

```rust
use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};
use codedb_core::{CodeDB, SearchType};

#[derive(Parser)]
#[command(name = "codedb", about = "Code indexing and search")]
struct Cli {
    #[arg(long, default_value = "~/.codedb")]
    root: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Clone and index a git repository
    Index {
        /// Repository URL
        url: String,
    },
    /// Search indexed code using Sourcegraph query syntax
    ///
    /// Supports filters: repo:, file:, -file:, lang:, type:, rev:, count:,
    /// author:, before:, after:, message:, select:
    ///
    /// Examples:
    ///   codedb search "process_data"
    ///   codedb search "lang:rust file:*.rs process_data"
    ///   codedb search "type:symbol lang:rust SFrame"
    ///   codedb search "type:diff author:ylow streaming"
    ///   codedb search "type:commit before:2026-01-01 refactor"
    Search {
        /// Search query (Sourcegraph syntax)
        query: String,

        /// Print generated SQL instead of executing
        #[arg(long)]
        sql: bool,
    },
    /// Run raw SQL query
    Sql {
        /// SQL query string
        query: String,
    },
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = expand_tilde(&cli.root);

    match cli.command {
        Commands::Index { url } => {
            let mut db = CodeDB::open(&root)?;
            println!("Indexing {url}...");
            db.index_repo(&url)?;
            println!("Parsing symbols...");
            let stats = db.parse_symbols()?;
            println!(
                "Done. Parsed {} blobs, extracted {} symbols.",
                stats.blobs_parsed, stats.symbols_extracted
            );
        }
        Commands::Search { query, sql: show_sql } => {
            let db = CodeDB::open(&root)?;

            if show_sql {
                let translated = db.translate_query(&query)?;
                println!("-- Sourcegraph query: {query}");
                println!("-- Parameters: {:?}", translated.params);
                println!("{}", translated.sql);
                return Ok(());
            }

            let results = db.search(&query)?;

            if results.rows.is_empty() {
                println!("No results found.");
                return Ok(());
            }

            for row in &results.rows {
                match results.search_type {
                    SearchType::Code => {
                        // Columns: path, score, snippet
                        let path = &row.columns[0].1;
                        let score = &row.columns[1].1;
                        let snippet = &row.columns[2].1;
                        println!("{path} (score: {score})");
                        println!("  {snippet}");
                        println!();
                    }
                    SearchType::Diff => {
                        // Columns: hash, message, path, score
                        let hash = &row.columns[0].1;
                        let message = &row.columns[1].1;
                        let path = &row.columns[2].1;
                        let score = &row.columns[3].1;
                        println!(
                            "{hash} {path} (score: {score})"
                        );
                        println!("  {message}");
                        println!();
                    }
                    SearchType::Commit => {
                        // Columns: hash, author, message
                        let hash = &row.columns[0].1;
                        let author = &row.columns[1].1;
                        let message = &row.columns[2].1;
                        println!("{hash} ({author}) {message}");
                    }
                    SearchType::Symbol => {
                        // Columns: path, name, kind, line
                        let path = &row.columns[0].1;
                        let name = &row.columns[1].1;
                        let kind = &row.columns[2].1;
                        let line = &row.columns[3].1;
                        println!("{path}:{line} {kind} {name}");
                    }
                }
            }
        }
        Commands::Sql { query } => {
            let db = CodeDB::open(&root)?;
            let mut stmt = db.conn().prepare(&query)?;
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap().to_string())
                .collect();
            println!("{}", col_names.join("\t"));

            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let vals: Vec<String> = (0..col_count)
                    .map(|i| {
                        row.get::<_, rusqlite::types::Value>(i)
                            .map(|v| format!("{v:?}"))
                            .unwrap_or_else(|_| "NULL".to_string())
                    })
                    .collect();
                println!("{}", vals.join("\t"));
            }
        }
    }

    Ok(())
}
```

**Step 2: Verify it builds**

Run: `cargo build -p codedb-cli 2>&1`

Expected: Compiles without errors.

**Step 3: Smoke test against existing demo data**

Run (assumes `/tmp/codedb-demo` exists from a previous demo run — if not, run `codedb index` first):

```bash
# Basic search (backward compatible)
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search "FlexType"

# With filters
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search "lang:rust file:*.rs serialize"

# Symbol search
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search "type:symbol lang:rust SFrame"

# Show SQL
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search --sql "type:symbol lang:rust SFrame"

# Diff search
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search "type:diff streaming"

# Commit search
cargo run -p codedb-cli --release -- --root /tmp/codedb-demo search "type:commit author:ylow"
```

Verify each produces reasonable output.

**Step 4: Commit**

```bash
git add codedb-cli/src/main.rs
git commit -m "feat(cli): upgrade search command with Sourcegraph query syntax and --sql flag"
```

---

### Task 7: Integration Test

**Files:**
- Modify: `codedb-core/tests/integration.rs`

**Context:** End-to-end test: index SFrameRust, then run Sourcegraph-style queries through `CodeDB::search()` and verify each search type returns results.

**Step 1: Add integration test**

Add to `codedb-core/tests/integration.rs`:

```rust
#[test]
fn test_sourcegraph_queries() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    db.index_repo("https://github.com/ylow/SFrameRust/").unwrap();
    db.parse_symbols().unwrap();

    // Code search — basic
    let results = db.search("FlexType").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Code search for FlexType should return results"
    );

    // Code search — with lang filter
    let results = db.search("lang:rust serialize").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Code search with lang:rust should return results"
    );

    // Code search — with file glob
    let results = db.search("file:*.rs struct").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Code search with file glob should return results"
    );

    // Symbol search
    let results = db.search("type:symbol lang:rust SFrame").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Symbol search should find SFrame"
    );

    // Symbol search with kind filter
    let results = db
        .search("type:symbol select:symbol.function lang:rust read")
        .unwrap();
    assert!(
        !results.rows.is_empty(),
        "Symbol search for functions named 'read' should return results"
    );

    // Diff search
    let results = db.search("type:diff streaming").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Diff search for 'streaming' should return results"
    );

    // Commit search
    let results = db.search("type:commit author:Yucheng").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Commit search by author should return results"
    );

    // translate_query returns SQL
    let translated = db.translate_query("lang:rust type:symbol foo").unwrap();
    assert!(translated.sql.contains("symbols"));
    assert!(translated.sql.contains("b.language"));
    assert!(!translated.params.is_empty());

    // Negative file filter
    let results = db.search("-file:test struct").unwrap();
    assert!(
        !results.rows.is_empty(),
        "Code search with -file:test should return results"
    );
}
```

**Step 2: Run the integration test**

Run: `cargo test -p codedb-core --test integration test_sourcegraph_queries -- --nocapture`

Expected: Test passes. This will take a few seconds since it indexes SFrameRust.

**Step 3: Run all tests**

Run: `cargo test --workspace -- --nocapture`

Expected: All tests pass (unit tests + integration tests).

**Step 4: Commit**

```bash
git add codedb-core/tests/integration.rs
git commit -m "test: add integration test for Sourcegraph query translator"
```
