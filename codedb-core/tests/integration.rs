use codedb_core::CodeDB;
use tempfile::TempDir;

#[test]
fn test_index_sframerust() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    db.index_repo("https://github.com/ylow/SFrameRust/")
        .unwrap();

    let conn = db.conn();

    // Verify repo was created
    let repo_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM repos", [], |r| r.get(0))
        .unwrap();
    assert_eq!(repo_count, 1);

    // Verify refs exist
    let ref_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM refs", [], |r| r.get(0))
        .unwrap();
    assert!(ref_count > 0, "Should have at least one ref");

    // Verify commits exist
    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert!(commit_count > 0, "Should have commits");

    // Verify blobs exist and are deduplicated
    let blob_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
        .unwrap();
    assert!(blob_count > 0, "Should have blobs");

    // Verify file_revs exist for at least one ref tip
    let file_rev_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_revs", [], |r| r.get(0))
        .unwrap();
    assert!(file_rev_count > 0, "Should have file_revs");

    // Verify code search works (SFrameRust is a Rust project with many 'fn' keywords)
    let search_results: Vec<(i64, f64)> = conn
        .prepare("SELECT blob_id, score FROM code_search('fn')")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!search_results.is_empty(), "Should find 'fn' in Rust code");

    // Verify diffs exist
    let diff_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM diffs", [], |r| r.get(0))
        .unwrap();
    assert!(diff_count > 0, "Should have diffs");

    // Verify join between code_search and file_revs works
    let joined: Vec<(String, f64)> = conn
        .prepare(
            "SELECT fr.path, cs.score
             FROM code_search('struct') cs
             JOIN blobs b ON b.id = cs.blob_id
             JOIN file_revs fr ON fr.blob_id = b.id
             JOIN refs r ON r.commit_id = fr.commit_id
             GROUP BY fr.path
             ORDER BY cs.score DESC
             LIMIT 5",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !joined.is_empty(),
        "Should find 'struct' in files via join"
    );
    // At least some paths should end with .rs (SFrameRust is a Rust project)
    let rs_count = joined.iter().filter(|(p, _)| p.ends_with(".rs")).count();
    assert!(
        rs_count > 0,
        "Expected at least one Rust file in results, got paths: {:?}",
        joined.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn test_incremental_update() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    // First index
    db.index_repo("https://github.com/ylow/SFrameRust/")
        .unwrap();

    let commit_count_1: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();

    // Re-index (should be incremental, no new commits)
    db.index_repo("https://github.com/ylow/SFrameRust/")
        .unwrap();

    let commit_count_2: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        commit_count_1, commit_count_2,
        "No duplicate commits after re-index"
    );
}

#[test]
fn test_symbol_extraction() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    db.index_repo("https://github.com/ylow/SFrameRust/")
        .unwrap();
    let stats = db.parse_symbols().unwrap();

    assert!(stats.blobs_parsed > 0, "Should have parsed some blobs");
    assert!(
        stats.symbols_extracted > 0,
        "Should have extracted symbols"
    );

    let conn = db.conn();

    // Verify symbols table populated
    let symbol_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert!(symbol_count > 0, "Should have symbols");

    // Verify symbol_refs table populated
    let ref_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbol_refs", [], |r| r.get(0))
        .unwrap();
    assert!(ref_count > 0, "Should have symbol refs");

    // Verify all supported-language blobs are marked as parsed
    let unparsed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM blobs WHERE parsed = 0 AND language IN ('rust', 'python', 'javascript', 'typescript', 'tsx', 'go', 'c', 'cpp')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unparsed, 0, "All supported-language blobs should be parsed");

    // Verify "what calls X" query pattern works
    let callers: Vec<(String, String)> = conn
        .prepare(
            "SELECT DISTINCT s.name, s.kind
             FROM symbol_refs sr
             JOIN symbols s ON s.id = sr.symbol_id
             WHERE sr.kind = 'call'
             LIMIT 10",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !callers.is_empty(),
        "Should find callers via symbol_refs join"
    );

    // Verify parent_id nesting works
    let nested_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE parent_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        nested_count > 0,
        "Should have nested symbols (methods inside impl blocks)"
    );

    // Run parse_symbols again — should be a no-op
    let stats2 = db.parse_symbols().unwrap();
    assert_eq!(stats2.blobs_parsed, 0, "Second run should parse 0 blobs");
}

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
