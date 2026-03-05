use std::sync::Arc;

use rusqlite::Connection;
use tantivy::schema::Field;
use tantivy::{Index, IndexReader};

use crate::types::{tantivy_type_to_sql, ColumnDef, ColumnSource};
use crate::vtab::{register_vtab, VTabState};
use crate::vtab_helpers::generate_ddl;

#[derive(Debug)]
pub enum BuildError {
    MissingIndex,
    MissingReader,
    NoSearchFields,
    NoColumns,
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
            BuildError::FieldNotStored(n) => write!(f, "field '{n}' is not stored"),
            BuildError::UnsupportedFieldType(n) => write!(f, "field '{n}' has unsupported type"),
            BuildError::SnippetFieldNotText(n) => write!(f, "snippet field '{n}' is not TEXT"),
            BuildError::DuplicateColumnName(n) => write!(f, "duplicate column name '{n}'"),
        }
    }
}

impl std::error::Error for BuildError {}

pub struct TantivyVTabBuilder {
    index: Option<Index>,
    reader: Option<IndexReader>,
    search_fields: Vec<Field>,
    columns: Vec<ColumnDef>,
    default_limit: usize,
}

impl Default for TantivyVTabBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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
            sql_type: "", // resolved during validate
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

    /// Validate, build, and register the virtual table in one step.
    pub fn register(self, conn: &Connection, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let validated = self.validate()?;
        validated.register(conn, name)?;
        Ok(())
    }

    /// Validate and finalize the builder, returning a ValidatedVTab ready for registration.
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

        // Validate and resolve SQL types for stored field columns
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
                        .ok_or(BuildError::UnsupportedFieldType(name))?;
                }
                ColumnSource::Snippet(field) => {
                    let entry = schema.get_field_entry(*field);
                    let name = entry.name().to_string();
                    if entry.field_type().value_type() != tantivy::schema::Type::Str {
                        return Err(BuildError::SnippetFieldNotText(name));
                    }
                }
                ColumnSource::Score => {}
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
        register_vtab(conn, name, state)
    }
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
    fn test_missing_index() {
        let result = TantivyVTabBuilder::new().validate();
        assert!(matches!(result, Err(BuildError::MissingIndex)));
    }

    #[test]
    fn test_missing_reader() {
        let (index, _id, body, _tag) = make_test_index();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .search_fields(vec![body])
            .validate();
        assert!(matches!(result, Err(BuildError::MissingReader)));
    }

    #[test]
    fn test_no_search_fields() {
        let (index, id, _body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![])
            .column("id", id)
            .validate();
        assert!(matches!(result, Err(BuildError::NoSearchFields)));
    }

    #[test]
    fn test_no_columns() {
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
    fn test_field_not_stored() {
        let (index, _id, body, tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("tag", tag)
            .validate();
        assert!(matches!(result, Err(BuildError::FieldNotStored(_))));
    }

    #[test]
    fn test_snippet_field_not_text() {
        let (index, id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("id", id)
            .snippet_column("snippet", id)
            .validate();
        assert!(matches!(result, Err(BuildError::SnippetFieldNotText(_))));
    }

    #[test]
    fn test_duplicate_column_name() {
        let (index, id, body, _tag) = make_test_index();
        let reader = index.reader().unwrap();
        let result = TantivyVTabBuilder::new()
            .index(index)
            .reader(reader)
            .search_fields(vec![body])
            .column("col", id)
            .column("col", body)
            .validate();
        assert!(matches!(result, Err(BuildError::DuplicateColumnName(_))));
    }

    #[test]
    fn test_valid_builder() {
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
        assert_eq!(vtab.columns[0].sql_type, "INTEGER");
        assert_eq!(vtab.columns[1].sql_type, "TEXT");
        assert_eq!(vtab.columns[2].sql_type, "REAL");
        assert_eq!(vtab.columns[3].sql_type, "TEXT");
    }
}
