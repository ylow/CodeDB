use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter};
use tantivy_sqlite::TantivyVTab;

use crate::query::{self, SearchResults, SearchResultRow, TranslatedQuery};
use crate::schema::init_schema;

pub struct CodeDB {
    root: PathBuf,
    conn: Connection,
    code_index: Index,
    diff_index: Index,
    code_reader: IndexReader,
    diff_reader: IndexReader,
    pub(crate) code_blob_id_field: Field,
    pub(crate) code_content_field: Field,
    pub(crate) diff_id_field: Field,
    pub(crate) diff_content_field: Field,
}

fn build_code_schema() -> (Schema, Field, Field) {
    let mut builder = Schema::builder();
    let blob_id = builder.add_u64_field("blob_id", STORED | FAST);
    let content = builder.add_text_field("content", TEXT | STORED);
    (builder.build(), blob_id, content)
}

fn build_diff_schema() -> (Schema, Field, Field) {
    let mut builder = Schema::builder();
    let diff_id = builder.add_u64_field("diff_id", STORED | FAST);
    let content = builder.add_text_field("diff_content", TEXT | STORED);
    (builder.build(), diff_id, content)
}

impl CodeDB {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root.join("tantivy/code_search"))?;
        std::fs::create_dir_all(root.join("tantivy/diff_search"))?;
        std::fs::create_dir_all(root.join("repos"))?;

        let db_path = root.join("db.sqlite");
        let conn =
            Connection::open(&db_path).context("Failed to open SQLite database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        init_schema(&conn)?;

        let (code_schema, code_blob_id_field, code_content_field) = build_code_schema();
        let code_index = if root.join("tantivy/code_search/meta.json").exists() {
            Index::open_in_dir(root.join("tantivy/code_search"))?
        } else {
            Index::create_in_dir(root.join("tantivy/code_search"), code_schema)?
        };
        let code_reader = code_index.reader()?;

        let (diff_schema, diff_id_field, diff_content_field) = build_diff_schema();
        let diff_index = if root.join("tantivy/diff_search/meta.json").exists() {
            Index::open_in_dir(root.join("tantivy/diff_search"))?
        } else {
            Index::create_in_dir(root.join("tantivy/diff_search"), diff_schema)?
        };
        let diff_reader = diff_index.reader()?;

        TantivyVTab::builder()
            .index(code_index.clone())
            .reader(code_reader.clone())
            .search_fields(vec![code_content_field])
            .column("blob_id", code_blob_id_field)
            .score_column("score")
            .snippet_column("snippet", code_content_field)
            .register(&conn, "code_search")
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("Failed to register code_search vtab")?;

        TantivyVTab::builder()
            .index(diff_index.clone())
            .reader(diff_reader.clone())
            .search_fields(vec![diff_content_field])
            .column("diff_id", diff_id_field)
            .score_column("score")
            .snippet_column("snippet", diff_content_field)
            .register(&conn, "diff_search")
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("Failed to register diff_search vtab")?;

        Ok(CodeDB {
            root: root.to_path_buf(),
            conn,
            code_index,
            diff_index,
            code_reader,
            diff_reader,
            code_blob_id_field,
            code_content_field,
            diff_id_field,
            diff_content_field,
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.root.join("repos")
    }

    pub fn code_writer(&self) -> Result<IndexWriter> {
        Ok(self.code_index.writer(50_000_000)?)
    }

    pub fn diff_writer(&self) -> Result<IndexWriter> {
        Ok(self.diff_index.writer(50_000_000)?)
    }

    pub fn reload_readers(&mut self) -> Result<()> {
        self.code_reader.reload()?;
        self.diff_reader.reload()?;
        Ok(())
    }

    /// Index a git repository by URL: clone/fetch, walk commits, populate DB and search indexes.
    ///
    /// `max_history_depth` limits how many commits are walked per ref.
    /// Pass `None` to index all reachable commits.
    pub fn index_repo(
        &mut self,
        url: &str,
        progress: Option<&dyn Fn(&str)>,
        max_history_depth: Option<usize>,
    ) -> Result<()> {
        crate::indexer::index_repo(self, url, progress, max_history_depth)
    }

    /// Parse symbols for all unparsed blobs that have a supported language.
    pub fn parse_symbols(&self, progress: Option<&dyn Fn(&str)>) -> Result<crate::symbols::ParseStats> {
        crate::symbols::parse_symbols(self.conn(), &self.repos_dir(), progress)
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_open_creates_directory_structure() {
        let tmp = TempDir::new().unwrap();
        let db = CodeDB::open(tmp.path()).unwrap();

        assert!(tmp.path().join("db.sqlite").exists());
        assert!(tmp.path().join("tantivy/code_search").exists());
        assert!(tmp.path().join("tantivy/diff_search").exists());
        assert!(tmp.path().join("repos").exists());

        // Verify vtabs are registered by running a query
        let mut stmt = db
            .conn()
            .prepare("SELECT blob_id FROM code_search('test')")
            .unwrap();
        let results: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_open_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let _db1 = CodeDB::open(tmp.path()).unwrap();
        drop(_db1);
        let _db2 = CodeDB::open(tmp.path()).unwrap();
    }

    #[test]
    fn test_translate_query() {
        let tmp = TempDir::new().unwrap();
        let db = CodeDB::open(tmp.path()).unwrap();
        let t = db.translate_query("lang:rust foo").unwrap();
        assert!(t.sql.contains("code_search"));
        assert!(t.sql.contains("b.language"));
    }
}
