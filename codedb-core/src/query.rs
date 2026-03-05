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
}
