use crate::types::ColumnDef;

pub const IDX_QUERY: i32 = 0x01;
pub const IDX_MODE: i32 = 0x02;
pub const IDX_LIMIT_COL: i32 = 0x04;

/// Generate the CREATE TABLE DDL for the virtual table.
pub fn generate_ddl(table_name: &str, columns: &[ColumnDef]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for col in columns {
        parts.push(format!("{} {}", col.name, col.sql_type));
    }

    parts.push("query TEXT HIDDEN".to_string());
    parts.push("mode TEXT HIDDEN".to_string());
    parts.push("query_limit INTEGER HIDDEN".to_string());

    format!("CREATE TABLE {}({})", table_name, parts.join(", "))
}

pub struct FilterArgs {
    pub has_query: bool,
    pub has_mode: bool,
    pub has_limit_col: bool,
}

pub fn decode_idx_num(idx_num: i32) -> FilterArgs {
    FilterArgs {
        has_query: idx_num & IDX_QUERY != 0,
        has_mode: idx_num & IDX_MODE != 0,
        has_limit_col: idx_num & IDX_LIMIT_COL != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnSource;

    #[test]
    fn test_generate_ddl() {
        let columns = vec![
            ColumnDef {
                name: "doc_id".to_string(),
                source: ColumnSource::Score,
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
            "CREATE TABLE my_search(doc_id INTEGER, body TEXT, score REAL, \
             query TEXT HIDDEN, mode TEXT HIDDEN, query_limit INTEGER HIDDEN)"
        );
    }

    #[test]
    fn test_idx_num_round_trip() {
        let idx = IDX_QUERY | IDX_MODE;
        let args = decode_idx_num(idx);
        assert!(args.has_query);
        assert!(args.has_mode);
        assert!(!args.has_limit_col);
    }

    #[test]
    fn test_idx_num_all_set() {
        let idx = IDX_QUERY | IDX_MODE | IDX_LIMIT_COL;
        let args = decode_idx_num(idx);
        assert!(args.has_query);
        assert!(args.has_mode);
        assert!(args.has_limit_col);
    }

    #[test]
    fn test_idx_num_none_set() {
        let args = decode_idx_num(0);
        assert!(!args.has_query);
        assert!(!args.has_mode);
        assert!(!args.has_limit_col);
    }
}
