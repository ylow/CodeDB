use tantivy::query::Query;
use tantivy::schema::Field;
use tantivy::Index;

#[derive(Debug)]
pub enum QueryBuildError {
    EmptyQuery,
    ParseError(String),
    UnknownMode(String),
}

impl std::fmt::Display for QueryBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryBuildError::EmptyQuery => write!(f, "query string is empty"),
            QueryBuildError::ParseError(e) => write!(f, "query parse error: {e}"),
            QueryBuildError::UnknownMode(m) => write!(f, "unknown query mode: '{m}'"),
        }
    }
}

impl std::error::Error for QueryBuildError {}

/// Build a Tantivy query from a query string and mode.
pub fn build_query(
    index: &Index,
    search_fields: &[Field],
    query_str: &str,
    mode: &str,
) -> Result<Box<dyn Query>, QueryBuildError> {
    if query_str.is_empty() {
        return Err(QueryBuildError::EmptyQuery);
    }

    match mode {
        "default" => {
            let parser = tantivy::query::QueryParser::for_index(index, search_fields.to_vec());
            parser
                .parse_query(query_str)
                .map_err(|e| QueryBuildError::ParseError(e.to_string()))
        }
        "regex" => {
            if search_fields.len() == 1 {
                let q = tantivy::query::RegexQuery::from_pattern(query_str, search_fields[0])
                    .map_err(|e| QueryBuildError::ParseError(e.to_string()))?;
                Ok(Box::new(q))
            } else {
                let mut subqueries: Vec<(tantivy::query::Occur, Box<dyn Query>)> = Vec::new();
                for &field in search_fields {
                    let q = tantivy::query::RegexQuery::from_pattern(query_str, field)
                        .map_err(|e| QueryBuildError::ParseError(e.to_string()))?;
                    subqueries.push((tantivy::query::Occur::Should, Box::new(q)));
                }
                Ok(Box::new(tantivy::query::BooleanQuery::new(subqueries)))
            }
        }
        "term" => {
            use tantivy::Term;
            if search_fields.len() == 1 {
                let term = Term::from_field_text(search_fields[0], query_str);
                Ok(Box::new(tantivy::query::TermQuery::new(
                    term,
                    tantivy::schema::IndexRecordOption::WithFreqs,
                )))
            } else {
                let mut subqueries: Vec<(tantivy::query::Occur, Box<dyn Query>)> = Vec::new();
                for &field in search_fields {
                    let term = Term::from_field_text(field, query_str);
                    let q = tantivy::query::TermQuery::new(
                        term,
                        tantivy::schema::IndexRecordOption::WithFreqs,
                    );
                    subqueries.push((tantivy::query::Occur::Should, Box::new(q)));
                }
                Ok(Box::new(tantivy::query::BooleanQuery::new(subqueries)))
            }
        }
        "phrase" => {
            let words: Vec<&str> = query_str.split_whitespace().collect();
            if words.is_empty() {
                return Err(QueryBuildError::EmptyQuery);
            }
            if search_fields.len() == 1 {
                let terms: Vec<tantivy::Term> = words
                    .iter()
                    .map(|w| tantivy::Term::from_field_text(search_fields[0], w))
                    .collect();
                Ok(Box::new(tantivy::query::PhraseQuery::new(terms)))
            } else {
                let mut subqueries: Vec<(tantivy::query::Occur, Box<dyn Query>)> = Vec::new();
                for &field in search_fields {
                    let terms: Vec<tantivy::Term> = words
                        .iter()
                        .map(|w| tantivy::Term::from_field_text(field, w))
                        .collect();
                    subqueries.push((
                        tantivy::query::Occur::Should,
                        Box::new(tantivy::query::PhraseQuery::new(terms)),
                    ));
                }
                Ok(Box::new(tantivy::query::BooleanQuery::new(subqueries)))
            }
        }
        other => Err(QueryBuildError::UnknownMode(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::schema::*;
    use tantivy::{doc, Index};

    fn make_test_index() -> (Index, Field) {
        let mut builder = Schema::builder();
        let body = builder.add_text_field("body", TEXT | STORED);
        let schema = builder.build();
        let index = Index::create_in_ram(schema);

        let mut writer = index.writer_with_num_threads(1, 15_000_000).unwrap();
        writer
            .add_document(doc!(body => "the quick brown fox jumps over the lazy dog"))
            .unwrap();
        writer.add_document(doc!(body => "hello world")).unwrap();
        writer.commit().unwrap();

        (index, body)
    }

    #[test]
    fn test_default_mode() {
        let (index, body) = make_test_index();
        assert!(build_query(&index, &[body], "fox", "default").is_ok());
    }

    #[test]
    fn test_regex_mode() {
        let (index, body) = make_test_index();
        assert!(build_query(&index, &[body], "fo.*", "regex").is_ok());
    }

    #[test]
    fn test_term_mode() {
        let (index, body) = make_test_index();
        assert!(build_query(&index, &[body], "fox", "term").is_ok());
    }

    #[test]
    fn test_phrase_mode() {
        let (index, body) = make_test_index();
        assert!(build_query(&index, &[body], "quick brown", "phrase").is_ok());
    }

    #[test]
    fn test_unknown_mode() {
        let (index, body) = make_test_index();
        assert!(matches!(
            build_query(&index, &[body], "fox", "magical"),
            Err(QueryBuildError::UnknownMode(_))
        ));
    }

    #[test]
    fn test_empty_query() {
        let (index, body) = make_test_index();
        assert!(matches!(
            build_query(&index, &[body], "", "default"),
            Err(QueryBuildError::EmptyQuery)
        ));
    }

    #[test]
    fn test_invalid_regex() {
        let (index, body) = make_test_index();
        assert!(matches!(
            build_query(&index, &[body], "[invalid", "regex"),
            Err(QueryBuildError::ParseError(_))
        ));
    }

    #[test]
    fn test_default_finds_documents() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "fox", "default").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let results = searcher
            .search(&query, &tantivy::collector::TopDocs::with_limit(10))
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_regex_finds_documents() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "hel.*", "regex").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let results = searcher
            .search(&query, &tantivy::collector::TopDocs::with_limit(10))
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_phrase_finds_exact_phrase() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "quick brown", "phrase").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let results = searcher
            .search(&query, &tantivy::collector::TopDocs::with_limit(10))
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_phrase_no_match_wrong_order() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "brown quick", "phrase").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let results = searcher
            .search(&query, &tantivy::collector::TopDocs::with_limit(10))
            .unwrap();
        assert_eq!(results.len(), 0);
    }
}
