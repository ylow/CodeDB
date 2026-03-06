use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::traverse::tree::Recorder;
use tantivy::doc;

use crate::codedb::CodeDB;
use crate::git_ops::{clone_or_fetch, repo_dir_from_url};
use crate::language::detect_language;

/// Derive a human-readable repo name from a URL.
/// e.g. "https://github.com/ylow/SFrameRust" -> "github.com/ylow/SFrameRust"
fn repo_name_from_url(url: &str) -> Result<String> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git://"))
        .unwrap_or(url);

    let cleaned = stripped.trim_end_matches('/').trim_end_matches(".git");
    if cleaned.is_empty() {
        anyhow::bail!("Invalid repo URL: {url}");
    }
    Ok(cleaned.to_string())
}

/// Walk a tree object and return a map of filepath -> blob OID for all blob entries.
fn get_tree_entries(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
) -> Result<HashMap<String, gix::ObjectId>> {
    let tree_obj = repo.find_object(tree_id)?;
    let tree = tree_obj.into_tree();
    let mut recorder = Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    let mut entries = HashMap::new();
    for entry in recorder.records {
        if entry.mode.is_blob() {
            let path = entry.filepath.to_str_lossy().into_owned();
            entries.insert(path, entry.oid);
        }
    }
    Ok(entries)
}

/// Index a git repository into the CodeDB database and search indexes.
///
/// This clones/fetches the repo, walks all refs and their commit histories,
/// and populates the SQLite tables and Tantivy indexes.
///
/// The optional `progress` callback receives status messages during indexing.
///
/// `max_history_depth` limits how many commits are walked per ref. If `None`,
/// all reachable commits are indexed. When the limit is hit, a warning is
/// reported via the progress callback but indexing continues with the
/// truncated history.
pub fn index_repo(
    db: &mut CodeDB,
    url: &str,
    progress: Option<&dyn Fn(&str)>,
    max_history_depth: Option<usize>,
) -> Result<()> {
    let report = |msg: &str| {
        if let Some(cb) = progress {
            cb(msg);
        }
    };
    // 1. Clone or fetch the repo
    report("Cloning/fetching repository...");
    let dir_name = repo_dir_from_url(url)?;
    let repo_path = db.repos_dir().join(&dir_name);
    let repo = clone_or_fetch(url, &repo_path)
        .with_context(|| format!("Failed to clone/fetch {url}"))?;

    // 2. Upsert into repos table, get repo_id
    let repo_name = repo_name_from_url(url)?;
    let repo_path_str = repo_path.to_string_lossy().to_string();
    db.conn().execute(
        "INSERT INTO repos (name, path) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET path = excluded.path",
        rusqlite::params![repo_name, repo_path_str],
    )?;
    let repo_id: i64 = db.conn().query_row(
        "SELECT id FROM repos WHERE name = ?1",
        rusqlite::params![repo_name],
        |row| row.get(0),
    )?;

    // 3. Load known commit hashes from DB into a HashSet
    let mut known_commits = HashSet::new();
    {
        let mut stmt = db
            .conn()
            .prepare("SELECT hash FROM commits WHERE repo_id = ?1")?;
        let rows = stmt.query_map(rusqlite::params![repo_id], |row| {
            row.get::<_, String>(0)
        })?;
        for hash in rows {
            known_commits.insert(hash?);
        }
    }

    // 4. Get Tantivy writers
    let mut code_writer = db.code_writer()?;
    let mut diff_writer = db.diff_writer()?;

    // 5. List all refs from gix repo
    report("Listing refs...");
    struct RefInfo {
        name: String,
        tip_oid: gix::ObjectId,
    }
    let mut ref_list = Vec::new();
    {
        let refs = repo.references()?;
        for reference in refs.all()?.flatten() {
            let name = reference.name().as_bstr().to_str_lossy().into_owned();
            // Try to peel to a commit; skip refs that don't point to commits
            let mut r = reference;
            match r.peel_to_id_in_place() {
                Ok(id) => {
                    ref_list.push(RefInfo {
                        name,
                        tip_oid: id.detach(),
                    });
                }
                Err(_) => {
                    // Skip refs that can't be peeled (e.g. broken refs)
                    continue;
                }
            }
        }
    }

    // 6. Begin SQLite transaction
    // We need to use execute_batch to start a transaction since conn() returns &Connection
    db.conn().execute_batch("BEGIN TRANSACTION")?;

    report(&format!("Found {} refs.", ref_list.len()));

    let result = (|| -> Result<()> {
        let mut total_new_commits = 0usize;
        let mut total_new_blobs = 0usize;
        let mut tree_cache: HashMap<gix::ObjectId, HashMap<String, gix::ObjectId>> = HashMap::new();

        // 7. For each ref
        for (ref_idx, ref_info) in ref_list.iter().enumerate() {
            // a. Walk ancestors from tip, stopping at known commits
            let mut walk_oids = vec![ref_info.tip_oid];
            let mut new_commits_data: Vec<(gix::ObjectId, CommitData)> = Vec::new();
            let mut visited = HashSet::new();

            let mut depth_truncated = false;
            while let Some(oid) = walk_oids.pop() {
                if let Some(max) = max_history_depth {
                    if new_commits_data.len() >= max {
                        depth_truncated = true;
                        break;
                    }
                }

                let oid_hex = oid.to_string();
                if known_commits.contains(&oid_hex) || !visited.insert(oid) {
                    continue;
                }

                let obj = repo.find_object(oid)?;
                let commit = obj.into_commit();
                let decoded = commit.decode()?;

                let author_name = decoded.author.name.to_str_lossy().into_owned();
                let message = decoded.message.to_str_lossy().into_owned();
                let timestamp = decoded.author.time.seconds;
                let tree_id = decoded.tree();
                let parent_ids: Vec<gix::ObjectId> = decoded.parents().collect();

                new_commits_data.push((
                    oid,
                    CommitData {
                        author: author_name,
                        message,
                        timestamp,
                        tree_id,
                        parent_ids,
                    },
                ));

                // Continue walking parents
                for parent_oid in &new_commits_data.last().unwrap().1.parent_ids {
                    walk_oids.push(*parent_oid);
                }
            }

            // b. Reverse to process oldest-first
            new_commits_data.reverse();

            if !new_commits_data.is_empty() {
                report(&format!(
                    "Ref {}/{}: {} — {} new commits",
                    ref_idx + 1,
                    ref_list.len(),
                    ref_info.name,
                    new_commits_data.len()
                ));
            }

            if depth_truncated {
                report(&format!(
                    "Warning: history depth limit ({}) reached for ref {}. \
                     Older commits will not be indexed. Use --depth to adjust.",
                    max_history_depth.unwrap(),
                    ref_info.name
                ));
            }

            // c. For each new commit
            for (oid, commit_data) in &new_commits_data {
                let oid_hex = oid.to_string();

                // INSERT OR IGNORE into commits table
                db.conn().execute(
                    "INSERT OR IGNORE INTO commits (repo_id, hash, author, message, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        repo_id,
                        oid_hex,
                        commit_data.author,
                        commit_data.message,
                        commit_data.timestamp
                    ],
                )?;

                let commit_db_id: i64 = db.conn().query_row(
                    "SELECT id FROM commits WHERE hash = ?1",
                    rusqlite::params![oid_hex],
                    |row| row.get(0),
                )?;

                total_new_commits += 1;
                if total_new_commits % 500 == 0 {
                    report(&format!("Processed {} commits, {} new blobs...", total_new_commits, total_new_blobs));
                }

                // INSERT OR IGNORE into commit_parents
                for parent_oid in &commit_data.parent_ids {
                    let parent_hex = parent_oid.to_string();
                    // Parent might not be in DB yet if it's from an older indexing run
                    // or if it was just inserted above
                    if let Ok(parent_db_id) = db.conn().query_row(
                        "SELECT id FROM commits WHERE hash = ?1",
                        rusqlite::params![parent_hex],
                        |row| row.get::<_, i64>(0),
                    ) {
                        db.conn().execute(
                            "INSERT OR IGNORE INTO commit_parents (commit_id, parent_id)
                             VALUES (?1, ?2)",
                            rusqlite::params![commit_db_id, parent_db_id],
                        )?;
                    }
                }

                // Compute diff: compare parent tree to this commit's tree
                // Use tree cache to avoid redundant tree walks
                if tree_cache.len() >= 32 {
                    tree_cache.clear();
                }
                if !tree_cache.contains_key(&commit_data.tree_id) {
                    let entries = get_tree_entries(&repo, commit_data.tree_id)?;
                    tree_cache.insert(commit_data.tree_id, entries);
                }
                let parent_tree_id = if let Some(parent_oid) = commit_data.parent_ids.first() {
                    let parent_obj = repo.find_object(*parent_oid)?;
                    let parent_commit = parent_obj.into_commit();
                    let parent_decoded = parent_commit.decode()?;
                    let ptid = parent_decoded.tree();
                    if !tree_cache.contains_key(&ptid) {
                        let entries = get_tree_entries(&repo, ptid)?;
                        tree_cache.insert(ptid, entries);
                    }
                    Some(ptid)
                } else {
                    None
                };
                let child_entries = tree_cache.get(&commit_data.tree_id).unwrap();
                let empty_tree = HashMap::new();
                let parent_entries = parent_tree_id
                    .and_then(|ptid| tree_cache.get(&ptid))
                    .unwrap_or(&empty_tree);

                // Find added, modified, and deleted files
                // Added or modified: files in child but not in parent, or with different OID
                for (path, child_blob_oid) in child_entries {
                    let is_changed = match parent_entries.get(path) {
                        None => true,                          // added
                        Some(parent_oid) => parent_oid != child_blob_oid, // modified
                    };
                    if !is_changed {
                        continue;
                    }

                    let old_blob_oid = parent_entries.get(path);

                    // Insert new blob
                    let (new_blob_db_id, is_new) =
                        ensure_blob(db, &repo, *child_blob_oid, path, &mut code_writer)?;
                    if is_new { total_new_blobs += 1; }

                    // Insert old blob if present
                    let old_blob_db_id = if let Some(&old_oid) = old_blob_oid {
                        let (id, is_new) = ensure_blob(db, &repo, old_oid, path, &mut code_writer)?;
                        if is_new { total_new_blobs += 1; }
                        Some(id)
                    } else {
                        None
                    };

                    // INSERT OR IGNORE diff record
                    db.conn().execute(
                        "INSERT OR IGNORE INTO diffs (commit_id, path, old_blob_id, new_blob_id)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![commit_db_id, path, old_blob_db_id, new_blob_db_id],
                    )?;

                    // Get diff DB id for Tantivy indexing
                    let diff_db_id: i64 = db.conn().query_row(
                        "SELECT id FROM diffs WHERE commit_id = ?1 AND path = ?2",
                        rusqlite::params![commit_db_id, path],
                        |row| row.get(0),
                    )?;

                    // Generate diff text and index in Tantivy
                    let diff_text = generate_diff_text(&repo, path, old_blob_oid, Some(child_blob_oid));
                    if !diff_text.is_empty() {
                        diff_writer.add_document(doc!(
                            db.diff_id_field => diff_db_id as u64,
                            db.diff_content_field => diff_text
                        ))?;
                    }
                }

                // Handle deleted files
                for (path, parent_blob_oid) in parent_entries {
                    if child_entries.contains_key(path) {
                        continue; // already handled above
                    }

                    // File was deleted
                    let (old_blob_db_id, is_new) =
                        ensure_blob(db, &repo, *parent_blob_oid, path, &mut code_writer)?;
                    if is_new { total_new_blobs += 1; }

                    db.conn().execute(
                        "INSERT OR IGNORE INTO diffs (commit_id, path, old_blob_id, new_blob_id)
                         VALUES (?1, ?2, ?3, NULL)",
                        rusqlite::params![commit_db_id, path, old_blob_db_id],
                    )?;

                    let diff_db_id: i64 = db.conn().query_row(
                        "SELECT id FROM diffs WHERE commit_id = ?1 AND path = ?2",
                        rusqlite::params![commit_db_id, path],
                        |row| row.get(0),
                    )?;

                    // Index deleted file content in diff search (no new blob)
                    let diff_text = generate_diff_text(&repo, path, Some(parent_blob_oid), None);
                    if !diff_text.is_empty() {
                        diff_writer.add_document(doc!(
                            db.diff_id_field => diff_db_id as u64,
                            db.diff_content_field => diff_text
                        ))?;
                    }
                }

                // Mark this commit as known
                known_commits.insert(oid_hex);
            }

            // d. Build file_revs for this ref's tip commit
            // First, resolve the tip commit's DB id
            let tip_hex = ref_info.tip_oid.to_string();
            if let Ok(tip_commit_db_id) = db.conn().query_row(
                "SELECT id FROM commits WHERE hash = ?1",
                rusqlite::params![tip_hex],
                |row| row.get::<_, i64>(0),
            ) {
                // Delete old file_revs for this commit (in case of re-index)
                db.conn().execute(
                    "DELETE FROM file_revs WHERE commit_id = ?1",
                    rusqlite::params![tip_commit_db_id],
                )?;

                // Walk the tree at the tip commit and insert file_revs
                let tip_obj = repo.find_object(ref_info.tip_oid)?;
                let tip_commit = tip_obj.into_commit();
                let tip_decoded = tip_commit.decode()?;
                let tip_tree_id = tip_decoded.tree();
                if !tree_cache.contains_key(&tip_tree_id) {
                    let entries = get_tree_entries(&repo, tip_tree_id)?;
                    tree_cache.insert(tip_tree_id, entries);
                }
                let tip_entries = tree_cache.get(&tip_tree_id).unwrap();

                for (path, blob_oid) in tip_entries {
                    let (blob_db_id, is_new) =
                        ensure_blob(db, &repo, *blob_oid, path, &mut code_writer)?;
                    if is_new { total_new_blobs += 1; }
                    db.conn().execute(
                        "INSERT OR IGNORE INTO file_revs (commit_id, path, blob_id)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![tip_commit_db_id, path, blob_db_id],
                    )?;
                }

                // e. Upsert refs table
                db.conn().execute(
                    "INSERT INTO refs (repo_id, name, commit_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT(repo_id, name) DO UPDATE SET commit_id = excluded.commit_id",
                    rusqlite::params![repo_id, ref_info.name, tip_commit_db_id],
                )?;
            }
        }

        report(&format!(
            "Indexing complete: {} new commits, {} new blobs.",
            total_new_commits, total_new_blobs
        ));
        Ok(())
    })();

    match result {
        Ok(()) => {
            report("Committing indexes...");
            // 8. Commit Tantivy writers
            code_writer.commit()?;
            diff_writer.commit()?;

            // 9. Commit SQLite transaction
            db.conn().execute_batch("COMMIT")?;

            // 10. Reload readers
            db.reload_readers()?;

            Ok(())
        }
        Err(e) => {
            // Rollback on error
            let _ = db.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Data extracted from a parsed commit.
struct CommitData {
    author: String,
    message: String,
    timestamp: i64,
    tree_id: gix::ObjectId,
    parent_ids: Vec<gix::ObjectId>,
}

/// Ensure a blob exists in the blobs table and Tantivy code index.
/// Returns (blob_db_id, is_new).
fn ensure_blob(
    db: &CodeDB,
    repo: &gix::Repository,
    blob_oid: gix::ObjectId,
    path: &str,
    code_writer: &mut tantivy::IndexWriter,
) -> Result<(i64, bool)> {
    let content_hash = blob_oid.to_string();
    let language = detect_language(path);

    // INSERT OR IGNORE — deduplicates by content_hash
    db.conn().execute(
        "INSERT OR IGNORE INTO blobs (content_hash, language) VALUES (?1, ?2)",
        rusqlite::params![content_hash, language],
    )?;

    let blob_db_id: i64 = db.conn().query_row(
        "SELECT id FROM blobs WHERE content_hash = ?1",
        rusqlite::params![content_hash],
        |row| row.get(0),
    )?;

    // Index in Tantivy only for newly inserted blobs (changes() > 0 means INSERT
    // happened rather than being ignored due to UNIQUE constraint).
    let is_new = db.conn().changes() > 0;
    if is_new {
        // New blob — read content and index in Tantivy
        if let Ok(obj) = repo.find_object(blob_oid) {
            if let Ok(text) = String::from_utf8(obj.data.clone()) {
                if !text.is_empty() {
                    code_writer.add_document(doc!(
                        db.code_blob_id_field => blob_db_id as u64,
                        db.code_content_field => text
                    ))?;
                }
            }
            // Skip binary files for Tantivy indexing
        }
    }

    Ok((blob_db_id, is_new))
}

/// Generate a simple diff text for indexing in Tantivy.
/// This is not a proper unified diff — just enough text for full-text search.
fn generate_diff_text(
    repo: &gix::Repository,
    path: &str,
    old_blob_oid: Option<&gix::ObjectId>,
    new_blob_oid: Option<&gix::ObjectId>,
) -> String {
    let mut diff_text = String::new();

    diff_text.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));

    // Get old content
    if let Some(&old_oid) = old_blob_oid {
        if let Ok(obj) = repo.find_object(old_oid) {
            if let Ok(text) = std::str::from_utf8(&obj.data) {
                for line in text.lines().take(100) {
                    diff_text.push_str(&format!("-{line}\n"));
                }
            }
        }
    }

    // Get new content
    if let Some(&new_oid) = new_blob_oid {
        if let Ok(obj) = repo.find_object(new_oid) {
            if let Ok(text) = std::str::from_utf8(&obj.data) {
                for line in text.lines().take(100) {
                    diff_text.push_str(&format!("+{line}\n"));
                }
            }
        }
    }

    diff_text
}
