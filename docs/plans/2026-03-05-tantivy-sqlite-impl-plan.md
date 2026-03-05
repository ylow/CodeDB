# tantivy-sqlite Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a generic Rust library that exposes any Tantivy index as a read-only SQLite virtual table via rusqlite's vtab API.

**Architecture:** Builder pattern to register a Tantivy index as an eponymous-only SQLite virtual table. The builder maps Tantivy fields to SQL columns. At query time, SQLite calls xBestIndex/xFilter which delegate to Tantivy search, returning results as virtual table rows.

**Tech Stack:** Rust, tantivy 0.22, rusqlite 0.32 (bundled, vtab features)

---

### Task 1: Workspace and crate scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `tantivy-sqlite/Cargo.toml`
- Create: `tantivy-sqlite/src/lib.rs`

**Step 1: Create workspace root Cargo.toml**

```toml
[workspace]
members = ["tantivy-sqlite"]
resolver = "2"
```

**Step 2: Create crate Cargo.toml**

```toml
[package]
name = "tantivy-sqlite"
version = "0.1.0"
edition = "2021"

[dependencies]
tantivy = "0.22"
rusqlite = { version = "0.32", features = ["bundled", "vtab"] }

[dev-dependencies]
tempfile = "3"
```

**Step 3: Create minimal lib.rs**

```rust
pub struct TantivyVTab;
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully (may take a while for first build with tantivy + sqlite)

**Step 5: Commit**

```bash
git add Cargo.toml tantivy-sqlite/
git commit -m "scaffold: tantivy-sqlite workspace and crate"
```

---

### Task 2: Core types and column definitions

Define the internal types for column mapping between Tantivy and SQLite.

**Files:**
- Create: `tantivy-sqlite/src/types.rs`
- Modify: `tantivy-sqlite/src/lib.rs`

**Step 1: Write tests for column type mapping**

Add to `tantivy-sqlite/src/types.rs`:

```rust
use tantivy::schema::{Field, Type};

/// How a virtual table column gets its value.
#[derive(Debug, Clone)]
pub enum ColumnSource {
    /// Value from a stored Tantivy field.
    StoredField(Field),
    /// BM25 relevance score.
    Score,
    /// Highlighted snippet from a stored TEXT field.
    Snippet(Field),
}

/// A column in the virtual table.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub source: ColumnSource,
    pub sql_type: &'static str,
}

/// Maps a Tantivy field type to a SQLite type name.
pub fn tantivy_type_to_sql(ty: Type) -> Option<&'static str> {
    match ty {
        Type::Str => Some("TEXT"),
        Type::U64 => Some("INTEGER"),
        Type::I64 => Some("INTEGER"),
        Type::F64 => Some("REAL"),
        Type::Bool => Some("INTEGER"),
        Type::Bytes => Some("BLOB"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tantivy_type_to_sql_mapping() {
        assert_eq!(tantivy_type_to_sql(Type::Str), Some("TEXT"));
        assert_eq!(tantivy_type_to_sql(Type::U64), Some("INTEGER"));
        assert_eq!(tantivy_type_to_sql(Type::I64), Some("INTEGER"));
        assert_eq!(tantivy_type_to_sql(Type::F64), Some("REAL"));
        assert_eq!(tantivy_type_to_sql(Type::Bool), Some("INTEGER"));
        assert_eq!(tantivy_type_to_sql(Type::Bytes), Some("BLOB"));
    }
}
```

**Step 2: Update lib.rs to declare module**

```rust
mod types;
pub use types::{ColumnDef, ColumnSource};
```

**Step 3: Run tests**

Run: `cargo test -p tantivy-sqlite`
Expected: 1 test passes

**Step 4: Commit**

```bash
git add tantivy-sqlite/src/
git commit -m "feat: add column type definitions and tantivy-to-sql type mapping"
```

---

### Task 3: Builder with validation

Implement the builder that collects column definitions and validates them against the Tantivy schema.

**Files:**
- Create: `tantivy-sqlite/src/builder.rs`
- Modify: `tantivy-sqlite/src/lib.rs`

**Step 1: Write failing tests for builder validation**

Add to `tantivy-sqlite/src/builder.rs`:

```rust
use tantivy::schema::Field;
use tantivy::{Index, IndexReader};
use crate::types::{ColumnDef, ColumnSource, tantivy_type_to_sql};

pub struct TantivyVTabBuilder {
    index: Option<Index>,
    reader: Option<IndexReader>,
    search_fields: Vec<Field>,
    columns: Vec<ColumnDef>,
    default_limit: usize,
}

#[derive(Debug)]
pub enum BuildError {
    MissingIndex,
    MissingReader,
    NoSearchFields,
    NoColumns,
    FieldNotInSchema(String),
    FieldNotStored(String),
    UnsupportedFieldType(String),
    SnippetFieldNotText(String),
    DuplicateColumnName(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::MissingIndex => write!(f, "index is required"),
            BuildError::MissingReader => write!(f, "reader is required"),
            BuildError::NoSearchFields => write!(f, "at least one search field is required"),
            BuildError::NoColumns => write!(f, "at least one column is required"),
            BuildError::FieldNotInSchema(n) => write!(f, "field '{}' not found in schema", n),
            BuildError::FieldNotStored(n) => write!(f, "field '{}' is not stored", n),
            BuildError::UnsupportedFieldType(n) => write!(f, "field '{}' has unsupported type", n),
            BuildError::SnippetFieldNotText(n) => write!(f, "snippet field '{}' is not TEXT", n),
            BuildError::DuplicateColumnName(n) => write!(f, "duplicate column name '{}'", n),
        }
    }
}

impl std::error::Error for BuildError {}

impl TantivyVTabBuilder {
    pub fn new() -> Self {
        Self {
            index: None,
            reader: None,
            search_fields: Vec::new(),
            columns: Vec::new(),
            default_limit: 1000,
        }
    }

    pub fn index(mut self, index: Index) -> Self {
        self.index = Some(index);
        self
    }

    pub fn reader(mut self, reader: IndexReader) -> Self {
        self.reader = Some(reader);
        self
    }

    pub fn search_fields(mut self, fields: Vec<Field>) -> Self {
        self.search_fields = fields;
        self
    }

    pub fn column(mut self, name: &str, field: Field) -> Self {
        self.columns.push(ColumnDef {
            name: name.to_string(),
            source: ColumnSource::StoredField(field),
            sql_type: "", // filled in during validate
        });
        self
    }

    pub fn score_column(mut self, name: &str) -> Self {
        self.columns.push(ColumnDef {
            name: name.to_string(),
            source: ColumnSource::Score,
            sql_type: "REAL",
        });
        self
    }

    pub fn snippet_column(mut self, name: &str, field: Field) -> Self {
        self.columns.push(ColumnDef {
            name: name.to_string(),
            source: ColumnSource::Snippet(field),
            sql_type: "TEXT",
        });
        self
    }

    pub fn default_limit(mut self, limit: usize) -> Self {
        self.default_limit = limit;
        self
    }

    /// Validate builder state and resolve SQL types for stored field columns.
    /// Returns validated (columns, search_fields, index, reader, default_limit).
    pub fn validate(mut self) -> Result<ValidatedVTab, BuildError> {
        let index = self.index.ok_or(BuildError::MissingIndex)?;
        let reader = self.reader.ok_or(BuildError::MissingReader)?;

        if self.search_fields.is_empty() {
            return Err(BuildError::NoSearchFields);
        }
        if self.columns.is_empty() {
            return Err(BuildError::NoColumns);
        }

        // Check for duplicate column names
        let mut seen = std::collections::HashSet::new();
        for col in &self.columns {
            if !seen.insert(&col.name) {
                return Err(BuildError::DuplicateColumnName(col.name.clone()));
            }
        }

        let schema = index.schema();

        // Validate and resolve types for stored field columns
        for col in &mut self.columns {
            match &col.source {
                ColumnSource::StoredField(field) => {
                    let entry = schema.get_field_entry(*field);
                    let name = entry.name().to_string();
                    if !entry.is_stored() {
                        return Err(BuildError::FieldNotStored(name));
                    }
                    let ty = entry.field_type().value_type();
                    col.sql_type = tantivy_type_to_sql(ty)
                        .ok_or_else(|| BuildError::UnsupportedFieldType(name))?;
                }
                ColumnSource::Snippet(field) => {
                    let entry = schema.get_field_entry(*field);
                    let name = entry.name().to_string();
                    if entry.field_type().value_type() != tantivy::schema::Type::Str {
                        return Err(BuildError::SnippetFieldNotText(name));
                    }
                }
                ColumnSource::Score => {
                    // Always valid
                }
            }
        }

        Ok(ValidatedVTab {
            index,
            reader,
            search_fields: self.search_fields,
            columns: self.columns,
            default_limit: self.default_limit,
        })
    }
}

pub struct ValidatedVTab {
    pub index: Index,
    pub reader: IndexReader,
    pub search_fields: Vec<Field>,
    pub columns: Vec<ColumnDef>,
    pub default_limit: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::schema::*;

    fn make_test_index() -> (Index, Field, Field, Field) {
        let mut builder = Schema::builder();
        let id_field = builder.add_u64_field("id", STORED);
        let body_field = builder.add_text_field("body", TEXT | STORED);
        let tag_field = builder.add_text_field("tag", TEXT); // not stored
        let schema = builder.build();
        let index = Index::create_in_ram(schema);
        (index, id_field, body_field, tag_field)
    }

    #[test]
    fn test_builder_missing_index() {
        let result = TantivyVTabBuilder::new()
            .search_fields(vec![])
            .validate();
        assert!(matches!(result, Err(BuildError::MissingIndex)));
    }

    #[test]
    fn test_builder_missing_reader() {
        let (index, _id, body, _tag) = make_test_index();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .search_fields(vec![body])
            .validate();
        assert!(matches!(result, Err(BuildError::MissingReader)));
    }

    #[test]
    fn test_builder_no_search_fields() {
        let (index, _id, _body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![])
            .column("id", _id)
            .validate();
        assert!(matches!(result, Err(BuildError::NoSearchFields)));
    }

    #[test]
    fn test_builder_no_columns() {
        let (index, _id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .validate();
        assert!(matches!(result, Err(BuildError::NoColumns)));
    }

    #[test]
    fn test_builder_field_not_stored() {
        let (index, _id, body, tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("tag", tag) // tag is not STORED
            .validate();
        assert!(matches!(result, Err(BuildError::FieldNotStored(_))));
    }

    #[test]
    fn test_builder_snippet_field_not_text() {
        let (index, id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("id", id)
            .snippet_column("snippet", id) // id is U64, not text
            .validate();
        assert!(matches!(result, Err(BuildError::SnippetFieldNotText(_))));
    }

    #[test]
    fn test_builder_duplicate_column_name() {
        let (index, id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("col", id)
            .column("col", body) // duplicate name
            .validate();
        assert!(matches!(result, Err(BuildError::DuplicateColumnName(_))));
    }

    #[test]
    fn test_builder_valid() {
        let (index, id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("id", id)
            .column("body", body)
            .score_column("score")
            .snippet_column("snippet", body)
            .default_limit(500)
            .validate();
        assert!(result.is_ok());
        let vtab = result.unwrap();
        assert_eq!(vtab.columns.len(), 4);
        assert_eq!(vtab.default_limit, 500);
        assert_eq!(vtab.columns[0].sql_type, "INTEGER"); // u64 -> INTEGER
        assert_eq!(vtab.columns[1].sql_type, "TEXT");     // text -> TEXT
        assert_eq!(vtab.columns[2].sql_type, "REAL");     // score -> REAL
        assert_eq!(vtab.columns[3].sql_type, "TEXT");     // snippet -> TEXT
    }
}
```

**Step 2: Update lib.rs**

```rust
mod types;
mod builder;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError, ValidatedVTab};

pub struct TantivyVTab;

impl TantivyVTab {
    pub fn builder() -> TantivyVTabBuilder {
        TantivyVTabBuilder::new()
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass (type tests + builder validation tests)

**Step 4: Commit**

```bash
git add tantivy-sqlite/src/
git commit -m "feat: add builder with validation for tantivy-sqlite vtab"
```

---

### Task 4: DDL generation and idxNum encoding

Generate the CREATE TABLE DDL string for SQLite and implement idxNum encoding/decoding.

**Files:**
- Create: `tantivy-sqlite/src/vtab_helpers.rs`
- Modify: `tantivy-sqlite/src/lib.rs`

**Step 1: Write tests for DDL generation and idxNum**

Add to `tantivy-sqlite/src/vtab_helpers.rs`:

```rust
use crate::types::ColumnDef;

/// Column indices for the hidden columns (appended after user columns).
/// query_col = columns.len()
/// mode_col  = columns.len() + 1
/// limit_col = columns.len() + 2

pub const IDX_QUERY: i32 = 0x01;
pub const IDX_MODE: i32 = 0x02;
pub const IDX_LIMIT_COL: i32 = 0x04;
pub const IDX_LIMIT_PUSHDOWN: i32 = 0x08;

/// Generate the CREATE TABLE DDL for the virtual table.
pub fn generate_ddl(table_name: &str, columns: &[ColumnDef]) -> String {
    let mut parts: Vec<String> = Vec::new();

    // User-defined columns
    for col in columns {
        parts.push(format!("{} {}", col.name, col.sql_type));
    }

    // Hidden columns for query, mode, limit
    parts.push("query TEXT HIDDEN".to_string());
    parts.push("mode TEXT HIDDEN".to_string());
    parts.push("query_limit INTEGER HIDDEN".to_string());

    format!("CREATE TABLE {}({})", table_name, parts.join(", "))
}

/// Decode idxNum into which arguments are present.
pub struct FilterArgs {
    pub has_query: bool,
    pub has_mode: bool,
    pub has_limit_col: bool,
    pub has_limit_pushdown: bool,
}

pub fn decode_idx_num(idx_num: i32) -> FilterArgs {
    FilterArgs {
        has_query: idx_num & IDX_QUERY != 0,
        has_mode: idx_num & IDX_MODE != 0,
        has_limit_col: idx_num & IDX_LIMIT_COL != 0,
        has_limit_pushdown: idx_num & IDX_LIMIT_PUSHDOWN != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnSource;
    use tantivy::schema::Field;

    #[test]
    fn test_generate_ddl() {
        let columns = vec![
            ColumnDef {
                name: "doc_id".to_string(),
                source: ColumnSource::Score, // source doesn't matter for DDL
                sql_type: "INTEGER",
            },
            ColumnDef {
                name: "body".to_string(),
                source: ColumnSource::Score,
                sql_type: "TEXT",
            },
            ColumnDef {
                name: "score".to_string(),
                source: ColumnSource::Score,
                sql_type: "REAL",
            },
        ];
        let ddl = generate_ddl("my_search", &columns);
        assert_eq!(
            ddl,
            "CREATE TABLE my_search(doc_id INTEGER, body TEXT, score REAL, query TEXT HIDDEN, mode TEXT HIDDEN, query_limit INTEGER HIDDEN)"
        );
    }

    #[test]
    fn test_idx_num_round_trip() {
        let idx = IDX_QUERY | IDX_MODE;
        let args = decode_idx_num(idx);
        assert!(args.has_query);
        assert!(args.has_mode);
        assert!(!args.has_limit_col);
        assert!(!args.has_limit_pushdown);

        let idx2 = IDX_QUERY | IDX_LIMIT_PUSHDOWN;
        let args2 = decode_idx_num(idx2);
        assert!(args2.has_query);
        assert!(!args2.has_mode);
        assert!(!args2.has_limit_col);
        assert!(args2.has_limit_pushdown);
    }

    #[test]
    fn test_idx_num_all_set() {
        let idx = IDX_QUERY | IDX_MODE | IDX_LIMIT_COL | IDX_LIMIT_PUSHDOWN;
        let args = decode_idx_num(idx);
        assert!(args.has_query);
        assert!(args.has_mode);
        assert!(args.has_limit_col);
        assert!(args.has_limit_pushdown);
    }
}
```

**Step 2: Update lib.rs**

```rust
mod types;
mod builder;
mod vtab_helpers;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError, ValidatedVTab};
// vtab_helpers is internal
```

**Step 3: Run tests**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tantivy-sqlite/src/
git commit -m "feat: add DDL generation and idxNum encoding for vtab"
```

---

### Task 5: Query building from mode string

Build the Tantivy query from a query string + mode.

**Files:**
- Create: `tantivy-sqlite/src/query_builder.rs`
- Modify: `tantivy-sqlite/src/lib.rs`

**Step 1: Write tests for query building**

```rust
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
            QueryBuildError::ParseError(e) => write!(f, "query parse error: {}", e),
            QueryBuildError::UnknownMode(m) => write!(f, "unknown query mode: '{}'", m),
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
            // Apply regex to each search field with BooleanQuery OR
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
            // Split query into terms and build a PhraseQuery for each search field
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

        // Add a document so queries have something to work with
        let mut writer = index.writer_with_num_threads(1, 10_000_000).unwrap();
        writer.add_document(doc!(body => "the quick brown fox jumps over the lazy dog")).unwrap();
        writer.add_document(doc!(body => "hello world")).unwrap();
        writer.commit().unwrap();

        (index, body)
    }

    #[test]
    fn test_default_mode() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "fox", "default");
        assert!(query.is_ok());
    }

    #[test]
    fn test_regex_mode() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "fo.*", "regex");
        assert!(query.is_ok());
    }

    #[test]
    fn test_term_mode() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "fox", "term");
        assert!(query.is_ok());
    }

    #[test]
    fn test_phrase_mode() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "quick brown", "phrase");
        assert!(query.is_ok());
    }

    #[test]
    fn test_unknown_mode() {
        let (index, body) = make_test_index();
        let result = build_query(&index, &[body], "fox", "magical");
        assert!(matches!(result, Err(QueryBuildError::UnknownMode(_))));
    }

    #[test]
    fn test_empty_query() {
        let (index, body) = make_test_index();
        let result = build_query(&index, &[body], "", "default");
        assert!(matches!(result, Err(QueryBuildError::EmptyQuery)));
    }

    #[test]
    fn test_invalid_regex() {
        let (index, body) = make_test_index();
        let result = build_query(&index, &[body], "[invalid", "regex");
        assert!(matches!(result, Err(QueryBuildError::ParseError(_))));
    }

    #[test]
    fn test_default_mode_actually_finds_documents() {
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
    fn test_regex_mode_actually_finds_documents() {
        let (index, body) = make_test_index();
        let query = build_query(&index, &[body], "hel.*", "regex").unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let results = searcher
            .search(&query, &tantivy::collector::TopDocs::with_limit(10))
            .unwrap();
        assert_eq!(results.len(), 1);
    }
}
```

**Step 2: Update lib.rs**

```rust
mod types;
mod builder;
mod vtab_helpers;
mod query_builder;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError, ValidatedVTab};
```

**Step 3: Run tests**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tantivy-sqlite/src/
git commit -m "feat: add query builder with mode-based dispatch (default, regex, term, phrase)"
```

---

### Task 6: VTab and VTabCursor implementation

Implement the rusqlite virtual table traits: `VTab` for `TantivyTable` and `VTabCursor` for `TantivyCursor`.

**Files:**
- Create: `tantivy-sqlite/src/vtab.rs`
- Modify: `tantivy-sqlite/src/lib.rs`
- Modify: `tantivy-sqlite/src/builder.rs` (add `register` method)

**Step 1: Implement VTab state and struct definitions**

In `tantivy-sqlite/src/vtab.rs`:

```rust
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::sync::Arc;

use rusqlite::ffi;
use rusqlite::types::Null;
use rusqlite::vtab::{
    eponymous_only_module, Context, IndexConstraintOp, IndexInfo, VTab, VTabCursor, Filters,
};

use tantivy::collector::TopDocs;
use tantivy::schema::Value;
use tantivy::snippet::SnippetGenerator;
use tantivy::{DocAddress, Index, IndexReader, Searcher, TantivyDocument};

use crate::query_builder::build_query;
use crate::types::{ColumnDef, ColumnSource};
use crate::vtab_helpers::*;

pub struct VTabState {
    pub index: Index,
    pub reader: IndexReader,
    pub search_fields: Vec<tantivy::schema::Field>,
    pub columns: Vec<ColumnDef>,
    pub ddl: String,
    pub default_limit: usize,
}

/// Column index offsets for hidden columns relative to user columns.
impl VTabState {
    pub fn query_col(&self) -> i32 {
        self.columns.len() as i32
    }
    pub fn mode_col(&self) -> i32 {
        self.columns.len() as i32 + 1
    }
    pub fn limit_col(&self) -> i32 {
        self.columns.len() as i32 + 2
    }
}

#[repr(C)]
pub struct TantivyTable {
    base: ffi::sqlite3_vtab,
    state: Arc<VTabState>,
}

/// A single search result with eagerly-fetched field values.
pub struct SearchResult {
    pub score: f32,
    pub field_values: Vec<Option<tantivy::schema::OwnedValue>>,
    pub snippet: Option<String>,
}

#[repr(C)]
pub struct TantivyCursor<'vtab> {
    base: ffi::sqlite3_vtab_cursor,
    state: Arc<VTabState>,
    results: Vec<SearchResult>,
    pos: usize,
    phantom: PhantomData<&'vtab TantivyTable>,
}

unsafe impl VTab for TantivyTable {
    type Aux = Arc<VTabState>;
    type Cursor = TantivyCursor<'static>;

    fn connect(
        _db: &mut rusqlite::vtab::VTabConnection,
        aux: Option<&Arc<VTabState>>,
        _args: &[&[u8]],
    ) -> rusqlite::Result<(String, Self)> {
        let state = aux.expect("VTabState aux data must be provided").clone();
        let ddl = state.ddl.clone();
        Ok((
            ddl,
            TantivyTable {
                base: ffi::sqlite3_vtab::default(),
                state,
            },
        ))
    }

    fn best_index(&self, info: &mut IndexInfo) -> rusqlite::Result<()> {
        let mut idx_num: i32 = 0;
        let mut argv_index: i32 = 1;

        let constraints: Vec<_> = info.constraints().collect();
        for (i, constraint) in constraints.iter().enumerate() {
            if !constraint.is_usable() {
                continue;
            }

            let col = constraint.column();
            let op = constraint.operator();

            if col == self.state.query_col() && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_QUERY;
                let mut usage = info.constraint_usage(i);
                usage.set_argv_index(argv_index);
                usage.set_omit(true);
                argv_index += 1;
            } else if col == self.state.mode_col()
                && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_MODE;
                let mut usage = info.constraint_usage(i);
                usage.set_argv_index(argv_index);
                usage.set_omit(true);
                argv_index += 1;
            } else if col == self.state.limit_col()
                && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_LIMIT_COL;
                let mut usage = info.constraint_usage(i);
                usage.set_argv_index(argv_index);
                usage.set_omit(true);
                argv_index += 1;
            }
            // Note: LIMIT pushdown via SQLITE_INDEX_CONSTRAINT_LIMIT is handled
            // if SQLite version supports it (3.38+). The constraint column will be -1.
        }

        if idx_num & IDX_QUERY != 0 {
            info.set_estimated_cost(100.0);
            info.set_estimated_rows(100);
        } else {
            // No query constraint — make this plan extremely expensive so SQLite
            // never chooses a full scan.
            info.set_estimated_cost(1e18);
            info.set_estimated_rows(i64::MAX);
        }

        info.set_idx_num(idx_num);
        Ok(())
    }

    fn open(&mut self) -> rusqlite::Result<TantivyCursor<'static>> {
        Ok(TantivyCursor {
            base: ffi::sqlite3_vtab_cursor::default(),
            state: self.state.clone(),
            results: Vec::new(),
            pos: 0,
            phantom: PhantomData,
        })
    }
}

unsafe impl VTabCursor for TantivyCursor<'_> {
    fn filter(
        &mut self,
        idx_num: c_int,
        _idx_str: Option<&str>,
        args: &Filters<'_>,
    ) -> rusqlite::Result<()> {
        self.results.clear();
        self.pos = 0;

        let flags = decode_idx_num(idx_num);
        if !flags.has_query {
            // No query provided — return empty result set
            return Ok(());
        }

        // Extract arguments in the order they were assigned in best_index
        let mut arg_idx = 0;

        let query_str: String = args.get(arg_idx)?;
        arg_idx += 1;

        let mode: String = if flags.has_mode {
            let m: String = args.get(arg_idx)?;
            arg_idx += 1;
            m
        } else {
            "default".to_string()
        };

        let limit: usize = if flags.has_limit_col {
            let l: i64 = args.get(arg_idx)?;
            // arg_idx += 1;  // not needed, last arg
            l.max(0) as usize
        } else if flags.has_limit_pushdown {
            let l: i64 = args.get(arg_idx)?;
            l.max(0) as usize
        } else {
            self.state.default_limit
        };

        // Build and execute the query
        let query = build_query(
            &self.state.index,
            &self.state.search_fields,
            &query_str,
            &mode,
        )
        .map_err(|e| {
            rusqlite::Error::ModuleError(e.to_string())
        })?;

        let searcher = self.state.reader.searcher();
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| rusqlite::Error::ModuleError(e.to_string()))?;

        // Build snippet generator if needed
        let snippet_col_field = self.state.columns.iter().find_map(|c| match &c.source {
            ColumnSource::Snippet(f) => Some(*f),
            _ => None,
        });
        let snippet_gen = snippet_col_field.and_then(|field| {
            SnippetGenerator::create(&searcher, &*query, field).ok()
        });

        // Eagerly fetch all results
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| rusqlite::Error::ModuleError(e.to_string()))?;

            let field_values: Vec<Option<tantivy::schema::OwnedValue>> = self
                .state
                .columns
                .iter()
                .map(|col| match &col.source {
                    ColumnSource::StoredField(field) => {
                        doc.get_first(*field).map(|v| v.into())
                    }
                    _ => None, // Score and Snippet handled separately
                })
                .collect();

            let snippet = snippet_gen.as_ref().map(|gen| {
                gen.snippet_from_doc(&doc).to_html()
            });

            self.results.push(SearchResult {
                score,
                field_values,
                snippet,
            });
        }

        Ok(())
    }

    fn next(&mut self) -> rusqlite::Result<()> {
        self.pos += 1;
        Ok(())
    }

    fn eof(&self) -> bool {
        self.pos >= self.results.len()
    }

    fn column(&self, ctx: &mut Context, col: c_int) -> rusqlite::Result<()> {
        let col_idx = col as usize;
        let result = &self.results[self.pos];

        // Hidden columns
        if col_idx >= self.state.columns.len() {
            // query, mode, limit — not meaningful to return
            ctx.set_result(&Null)?;
            return Ok(());
        }

        let col_def = &self.state.columns[col_idx];
        match &col_def.source {
            ColumnSource::Score => {
                ctx.set_result(&(result.score as f64))?;
            }
            ColumnSource::Snippet(_) => {
                match &result.snippet {
                    Some(s) => ctx.set_result(&s.as_str())?,
                    None => ctx.set_result(&Null)?,
                }
            }
            ColumnSource::StoredField(_) => {
                match &result.field_values[col_idx] {
                    Some(val) => set_owned_value(ctx, val)?,
                    None => ctx.set_result(&Null)?,
                }
            }
        }
        Ok(())
    }

    fn rowid(&self) -> rusqlite::Result<i64> {
        Ok(self.pos as i64)
    }
}

/// Convert a Tantivy OwnedValue to a SQLite result via the Context.
fn set_owned_value(ctx: &mut Context, val: &tantivy::schema::OwnedValue) -> rusqlite::Result<()> {
    use tantivy::schema::OwnedValue;
    match val {
        OwnedValue::Str(s) => ctx.set_result(&s.as_str())?,
        OwnedValue::U64(n) => ctx.set_result(&(*n as i64))?,
        OwnedValue::I64(n) => ctx.set_result(n)?,
        OwnedValue::F64(n) => ctx.set_result(n)?,
        OwnedValue::Bool(b) => ctx.set_result(&(*b as i32))?,
        OwnedValue::Bytes(b) => ctx.set_result(&b.as_slice())?,
        _ => ctx.set_result(&Null)?,
    }
    Ok(())
}
```

**Step 2: Add `register` method to builder**

In `tantivy-sqlite/src/builder.rs`, add to `ValidatedVTab`:

```rust
use std::sync::Arc;
use rusqlite::Connection;
use crate::vtab::{VTabState, TantivyTable};
use crate::vtab_helpers::generate_ddl;

impl ValidatedVTab {
    pub fn register(self, conn: &Connection, name: &str) -> Result<(), rusqlite::Error> {
        let state = Arc::new(VTabState {
            ddl: generate_ddl(name, &self.columns),
            index: self.index,
            reader: self.reader,
            search_fields: self.search_fields,
            columns: self.columns,
            default_limit: self.default_limit,
        });

        conn.create_module(
            name,
            rusqlite::vtab::eponymous_only_module::<TantivyTable>(),
            Some(state),
        )
    }
}
```

**Step 3: Update lib.rs and builder.rs public API**

Update `lib.rs`:
```rust
mod types;
mod builder;
mod vtab_helpers;
mod query_builder;
mod vtab;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError};

pub struct TantivyVTab;

impl TantivyVTab {
    pub fn builder() -> TantivyVTabBuilder {
        TantivyVTabBuilder::new()
    }
}
```

Also update the builder's `validate` to return a type that has `register`:
- `TantivyVTabBuilder` gains a `register` convenience method that calls `validate()?.register(conn, name)`

Add to `TantivyVTabBuilder`:
```rust
pub fn register(self, conn: &Connection, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let validated = self.validate()?;
    validated.register(conn, name)?;
    Ok(())
}
```

**Step 4: Run tests (existing should still pass)**

Run: `cargo test -p tantivy-sqlite`
Expected: All previous tests pass, no new tests yet (integration tests next)

**Step 5: Commit**

```bash
git add tantivy-sqlite/src/
git commit -m "feat: implement VTab and VTabCursor traits for tantivy-sqlite"
```

---

### Task 7: Integration tests — basic search

End-to-end tests: create index, register vtab, query from SQL.

**Files:**
- Create: `tantivy-sqlite/tests/integration.rs`

**Step 1: Write integration test for basic search**

```rust
use rusqlite::Connection;
use tantivy::schema::*;
use tantivy::{doc, Index};
use tantivy_sqlite::TantivyVTab;

fn setup() -> (Connection, Index, Field, Field) {
    let mut builder = Schema::builder();
    let id_field = builder.add_u64_field("id", STORED | FAST);
    let body_field = builder.add_text_field("body", TEXT | STORED);
    let schema = builder.build();
    let index = Index::create_in_ram(schema);

    let mut writer = index.writer_with_num_threads(1, 10_000_000).unwrap();
    writer.add_document(doc!(id_field => 1u64, body_field => "the quick brown fox jumps over the lazy dog")).unwrap();
    writer.add_document(doc!(id_field => 2u64, body_field => "the quick brown cat sits on the mat")).unwrap();
    writer.add_document(doc!(id_field => 3u64, body_field => "hello world from rust")).unwrap();
    writer.commit().unwrap();

    let conn = Connection::open_in_memory().unwrap();
    (conn, index, id_field, body_field)
}

#[test]
fn test_basic_search() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .column("body", body_field)
        .score_column("score")
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id, body, score FROM search('fox')")
        .unwrap();
    let results: Vec<(i64, String, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, 1); // doc id
    assert!(results[0].1.contains("fox"));
    assert!(results[0].2 > 0.0); // score > 0
}

#[test]
fn test_multiple_results() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .score_column("score")
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id, score FROM search('quick brown')")
        .unwrap();
    let results: Vec<(i64, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    // Both docs 1 and 2 contain "quick" and "brown"
    assert_eq!(results.len(), 2);
}

#[test]
fn test_no_results() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('nonexistent')")
        .unwrap();
    let results: Vec<i64> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert!(results.is_empty());
}

#[test]
fn test_score_ordering() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .score_column("score")
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id, score FROM search('quick brown') ORDER BY score DESC")
        .unwrap();
    let results: Vec<(i64, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert!(results.len() >= 2);
    // Scores should be in descending order
    for window in results.windows(2) {
        assert!(window[0].1 >= window[1].1);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass

**Step 3: Commit**

```bash
git add tantivy-sqlite/tests/
git commit -m "test: add basic integration tests for tantivy-sqlite vtab"
```

---

### Task 8: Integration tests — query modes, snippets, joins, errors

**Files:**
- Modify: `tantivy-sqlite/tests/integration.rs`

**Step 1: Add regex mode test**

```rust
#[test]
fn test_regex_mode() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('hel.*', 'regex')")
        .unwrap();
    let results: Vec<i64> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], 3); // "hello world from rust"
}
```

**Step 2: Add phrase mode test**

```rust
#[test]
fn test_phrase_mode() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('brown fox', 'phrase')")
        .unwrap();
    let results: Vec<i64> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], 1); // only doc 1 has "brown fox" as a phrase
}
```

**Step 3: Add snippet test**

```rust
#[test]
fn test_snippet() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .snippet_column("snippet", body_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id, snippet FROM search('fox')")
        .unwrap();
    let results: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    // Snippet should contain HTML highlighting
    assert!(results[0].1.contains("<b>fox</b>"));
}
```

**Step 4: Add JOIN test**

```rust
#[test]
fn test_join_with_regular_table() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .score_column("score")
        .register(&conn, "search")
        .unwrap();

    // Create a regular table with metadata
    conn.execute_batch(
        "CREATE TABLE docs (id INTEGER PRIMARY KEY, path TEXT, language TEXT);
         INSERT INTO docs VALUES (1, 'animals.txt', 'english');
         INSERT INTO docs VALUES (2, 'pets.txt', 'english');
         INSERT INTO docs VALUES (3, 'hello.rs', 'rust');",
    )
    .unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT d.path, d.language, s.score
             FROM search('fox') s
             JOIN docs d ON d.id = s.id
             WHERE d.language = 'english'
             ORDER BY s.score DESC",
        )
        .unwrap();
    let results: Vec<(String, String, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "animals.txt");
}
```

**Step 5: Add error handling test**

```rust
#[test]
fn test_bad_query_returns_error() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('[invalid', 'regex')")
        .unwrap();
    let result: rusqlite::Result<Vec<i64>> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect();

    // Should get an error, not a panic
    assert!(result.is_err());
}

#[test]
fn test_unknown_mode_returns_error() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('fox', 'telekinesis')")
        .unwrap();
    let result: rusqlite::Result<Vec<i64>> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect();

    assert!(result.is_err());
}
```

**Step 6: Add LIMIT test**

```rust
#[test]
fn test_sql_limit() {
    let (conn, index, id_field, body_field) = setup();
    let reader = index.reader().unwrap();

    TantivyVTab::builder()
        .index(index)
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .register(&conn, "search")
        .unwrap();

    let mut stmt = conn
        .prepare("SELECT id FROM search('quick brown') LIMIT 1")
        .unwrap();
    let results: Vec<i64> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
}
```

**Step 7: Run all tests**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass

**Step 8: Commit**

```bash
git add tantivy-sqlite/tests/
git commit -m "test: add query mode, snippet, join, error handling, and limit tests"
```

---

### Task 9: Polish and final verification

**Files:**
- Modify: `tantivy-sqlite/src/lib.rs` (clean up public API)

**Step 1: Ensure public API is clean**

`lib.rs` should export only what users need:
```rust
mod types;
mod builder;
mod vtab_helpers;
mod query_builder;
mod vtab;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError};

pub struct TantivyVTab;

impl TantivyVTab {
    pub fn builder() -> TantivyVTabBuilder {
        TantivyVTabBuilder::new()
    }
}
```

**Step 2: Run full test suite**

Run: `cargo test -p tantivy-sqlite`
Expected: All tests pass

**Step 3: Run clippy**

Run: `cargo clippy -p tantivy-sqlite -- -D warnings`
Expected: No warnings

**Step 4: Commit and push**

```bash
git add -A
git commit -m "chore: polish public API for tantivy-sqlite"
git push
```

---

### Task Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1    | Workspace + crate scaffold | — |
| 2    | Core types + column defs | 1 |
| 3    | Builder with validation | 2 |
| 4    | DDL generation + idxNum encoding | 2 |
| 5    | Query building from mode | 1 |
| 6    | VTab + VTabCursor implementation | 3, 4, 5 |
| 7    | Integration tests — basic search | 6 |
| 8    | Integration tests — modes, snippets, joins, errors | 7 |
| 9    | Polish + final verification | 8 |

Tasks 2–5 could be parallelized but are sequenced for clarity.
