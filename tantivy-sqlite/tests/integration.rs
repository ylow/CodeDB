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

    let mut writer = index.writer_with_num_threads(1, 15_000_000).unwrap();
    writer
        .add_document(
            doc!(id_field => 1u64, body_field => "the quick brown fox jumps over the lazy dog"),
        )
        .unwrap();
    writer
        .add_document(
            doc!(id_field => 2u64, body_field => "the quick brown cat sits on the mat"),
        )
        .unwrap();
    writer
        .add_document(doc!(id_field => 3u64, body_field => "hello world from rust"))
        .unwrap();
    writer.commit().unwrap();

    let conn = Connection::open_in_memory().unwrap();
    (conn, index, id_field, body_field)
}

fn register(conn: &Connection, index: &Index, id_field: Field, body_field: Field) {
    let reader = index.reader().unwrap();
    TantivyVTab::builder()
        .index(index.clone())
        .reader(reader)
        .search_fields(vec![body_field])
        .column("id", id_field)
        .column("body", body_field)
        .score_column("score")
        .register(conn, "search")
        .unwrap();
}

#[test]
fn test_basic_search() {
    let (conn, index, id_field, body_field) = setup();
    register(&conn, &index, id_field, body_field);

    let mut stmt = conn
        .prepare("SELECT id, body, score FROM search('fox')")
        .unwrap();
    let results: Vec<(i64, String, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, 1);
    assert!(results[0].1.contains("fox"));
    assert!(results[0].2 > 0.0);
}

#[test]
fn test_multiple_results() {
    let (conn, index, id_field, body_field) = setup();
    register(&conn, &index, id_field, body_field);

    let mut stmt = conn
        .prepare("SELECT id FROM search('quick brown')")
        .unwrap();
    let results: Vec<i64> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(results.len(), 2);
}

#[test]
fn test_no_results() {
    let (conn, index, id_field, body_field) = setup();
    register(&conn, &index, id_field, body_field);

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
    register(&conn, &index, id_field, body_field);

    let mut stmt = conn
        .prepare("SELECT id, score FROM search('quick brown') ORDER BY score DESC")
        .unwrap();
    let results: Vec<(i64, f64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert!(results.len() >= 2);
    for window in results.windows(2) {
        assert!(window[0].1 >= window[1].1);
    }
}
