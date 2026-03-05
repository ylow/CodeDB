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
