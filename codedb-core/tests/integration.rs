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
