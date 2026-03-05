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
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                chars.next();
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
        let (negated, rest) = if let Some(r) = token.strip_prefix('-') {
            (true, r)
        } else {
            (false, token.as_str())
        };

        if let Some((key, value)) = rest.split_once(':') {
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
    let sql = sql
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(TranslatedQuery {
        sql,
        params: p.params,
        search_type: SearchType::Code,
    })
}

fn translate_diff(_query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Implemented in Task 4")
}

fn translate_commit(_query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Implemented in Task 4")
}

fn translate_symbol(_query: &ParsedQuery) -> Result<TranslatedQuery> {
    todo!("Implemented in Task 4")
}

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
        assert_eq!(tokenize("-file:test foo"), vec!["-file:test", "foo"]);
    }

    #[test]
    fn test_tokenize_empty() {
        assert_eq!(tokenize(""), Vec::<String>::new());
    }

    #[test]
    fn test_tokenize_extra_whitespace() {
        assert_eq!(tokenize("  foo   bar  "), vec!["foo", "bar"]);
    }

    #[test]
    fn test_tokenize_unclosed_quote() {
        // Unclosed quote consumes to end of input
        assert_eq!(tokenize("foo \"bar baz"), vec!["foo", "\"bar baz\""]);
    }

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
        let q = parse_query("type:commit before:2026-01-01 after:2025-06-01 refactor").unwrap();
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
        let q = parse_query("type:symbol select:symbol.function foo").unwrap();
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
        assert!(err.to_string().contains("Unknown search type"), "got: {err}");
    }

    #[test]
    fn test_parse_invalid_count_error() {
        let err = parse_query("count:abc foo").unwrap_err();
        assert!(err.to_string().contains("positive integer"), "got: {err}");
    }

    #[test]
    fn test_parse_negation_unsupported() {
        let err = parse_query("-repo:foo bar").unwrap_err();
        assert!(err.to_string().contains("Negation not supported"), "got: {err}");
    }

    #[test]
    fn test_parse_unknown_filter_is_search_term() {
        let q = parse_query("http://example.com foo").unwrap();
        assert_eq!(q.search_pattern, "http://example.com foo");
    }

    #[test]
    fn test_parse_lang_alias() {
        let q = parse_query("l:python foo").unwrap();
        assert_eq!(q.filters.lang.as_deref(), Some("python"));
    }

    // --- SQL generation tests ---

    #[test]
    fn test_translate_code_basic() {
        let q = parse_query("process_data").unwrap();
        let t = translate(&q).unwrap();
        assert_eq!(t.search_type, SearchType::Code);
        assert!(t.sql.contains("code_search(?1)"));
        assert!(t.sql.contains("LIMIT 20"));
        assert_eq!(t.params[0], "process_data");
        assert!(t.params.iter().any(|p| p.contains("refs/heads/main")));
    }

    #[test]
    fn test_translate_code_with_filters() {
        let q = parse_query("lang:rust file:*.rs count:10 foo").unwrap();
        let t = translate(&q).unwrap();
        assert!(t.sql.contains("b.language ="));
        assert!(t.sql.contains("fr.path GLOB"));
        assert!(t.sql.contains("LIMIT 10"));
        assert_eq!(t.params[0], "foo");
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
        assert!(err.to_string().contains("requires a search pattern"), "got: {err}");
    }

    #[test]
    fn test_translate_code_substring_match() {
        let q = parse_query("file:csv foo").unwrap();
        let t = translate(&q).unwrap();
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
}
