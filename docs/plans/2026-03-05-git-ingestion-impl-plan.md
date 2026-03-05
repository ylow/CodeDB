# Git Ingestion Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build `codedb-core` library and `codedb-cli` binary that clone remote git repos (bare), walk their history, and populate SQLite + Tantivy indexes.

**Architecture:** `CodeDB::open()` creates/opens a data directory with SQLite DB and Tantivy indexes. `CodeDB::index_repo(url)` clones bare (or fetches), walks commits and diffs via gix, populates all tables, indexes blob content and diff text in Tantivy. The tantivy-sqlite vtab bridge enables SQL queries that join Tantivy search results with metadata.

**Tech Stack:** Rust, gix (gitoxide), rusqlite (bundled), tantivy, tantivy-sqlite

---

### Task 1: Scaffold codedb-core and codedb-cli crates

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `codedb-core/Cargo.toml`
- Create: `codedb-core/src/lib.rs`
- Create: `codedb-cli/Cargo.toml`
- Create: `codedb-cli/src/main.rs`

**Step 1: Create crate files**

`Cargo.toml` (workspace root):
```toml
[workspace]
members = ["tantivy-sqlite", "codedb-core", "codedb-cli"]
resolver = "2"
```

`codedb-core/Cargo.toml`:
```toml
[package]
name = "codedb-core"
version = "0.1.0"
edition = "2021"

[dependencies]
tantivy = "0.22"
tantivy-sqlite = { path = "../tantivy-sqlite" }
rusqlite = { version = "0.32", features = ["bundled", "vtab"] }
gix = { version = "0.70", features = ["blocking-network-client"] }
anyhow = "1"
```

Note: gix version may need adjustment based on what resolves. Start with 0.70
and bump if needed.

`codedb-core/src/lib.rs`:
```rust
pub struct CodeDB;
```

`codedb-cli/Cargo.toml`:
```toml
[package]
name = "codedb-cli"
version = "0.1.0"
edition = "2021"

[dependencies]
codedb-core = { path = "../codedb-core" }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```

`codedb-cli/src/main.rs`:
```rust
fn main() {
    println!("codedb-cli placeholder");
}
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles (may take a while for gix download)

**Step 3: Commit**

```bash
git add Cargo.toml codedb-core/ codedb-cli/
git commit -m "scaffold: add codedb-core and codedb-cli crates"
```

---

### Task 2: SQLite schema initialization

Create the schema from DESIGN.md and a function to initialize it.

**Files:**
- Create: `codedb-core/src/schema.rs`
- Modify: `codedb-core/src/lib.rs`

**Step 1: Write test for schema creation**

In `codedb-core/src/schema.rs`:

```rust
use rusqlite::Connection;

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA_SQL)
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS repos (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS refs (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    name      TEXT NOT NULL,
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    UNIQUE(repo_id, name)
);

CREATE TABLE IF NOT EXISTS commits (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    hash      TEXT NOT NULL UNIQUE,
    author    TEXT,
    message   TEXT,
    timestamp INTEGER
);

CREATE TABLE IF NOT EXISTS commit_parents (
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    parent_id INTEGER NOT NULL REFERENCES commits(id),
    PRIMARY KEY (commit_id, parent_id)
);

CREATE TABLE IF NOT EXISTS blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    language     TEXT
);

CREATE TABLE IF NOT EXISTS file_revs (
    id        INTEGER PRIMARY KEY,
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    path      TEXT NOT NULL,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    UNIQUE(commit_id, path)
);

CREATE TABLE IF NOT EXISTS diffs (
    id          INTEGER PRIMARY KEY,
    commit_id   INTEGER NOT NULL REFERENCES commits(id),
    path        TEXT NOT NULL,
    old_blob_id INTEGER REFERENCES blobs(id),
    new_blob_id INTEGER REFERENCES blobs(id),
    UNIQUE(commit_id, path)
);

CREATE TABLE IF NOT EXISTS symbols (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    parent_id INTEGER REFERENCES symbols(id),
    name      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL,
    end_line  INTEGER,
    end_col   INTEGER
);

CREATE TABLE IF NOT EXISTS symbol_refs (
    id        INTEGER PRIMARY KEY,
    blob_id   INTEGER NOT NULL REFERENCES blobs(id),
    symbol_id INTEGER REFERENCES symbols(id),
    ref_name  TEXT NOT NULL,
    kind      TEXT NOT NULL,
    line      INTEGER NOT NULL,
    col       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_commits_repo ON commits(repo_id);
CREATE INDEX IF NOT EXISTS idx_refs_repo ON refs(repo_id);
CREATE INDEX IF NOT EXISTS idx_file_revs_commit ON file_revs(commit_id);
CREATE INDEX IF NOT EXISTS idx_file_revs_blob ON file_revs(blob_id);
CREATE INDEX IF NOT EXISTS idx_diffs_commit ON diffs(commit_id);
CREATE INDEX IF NOT EXISTS idx_symbols_blob ON symbols(blob_id);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbol_refs_blob ON symbol_refs(blob_id);
CREATE INDEX IF NOT EXISTS idx_symbol_refs_name ON symbol_refs(ref_name);
CREATE INDEX IF NOT EXISTS idx_symbol_refs_symbol ON symbol_refs(symbol_id);
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_schema() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        // Verify tables exist by inserting a repo
        conn.execute(
            "INSERT INTO repos (name, path) VALUES ('test', '/tmp/test')",
            [],
        ).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_init_schema_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap(); // should not error
    }
}
```

**Step 2: Update lib.rs**

```rust
pub mod schema;
pub struct CodeDB;
```

**Step 3: Run tests**

Run: `cargo test -p codedb-core`
Expected: 2 tests pass

**Step 4: Commit**

```bash
git add codedb-core/src/
git commit -m "feat: add SQLite schema initialization for codedb-core"
```

---

### Task 3: CodeDB struct with open/create

Implement `CodeDB::open()` which creates the directory layout, opens SQLite,
creates Tantivy indexes, and registers the virtual tables.

**Files:**
- Create: `codedb-core/src/codedb.rs`
- Modify: `codedb-core/src/lib.rs`

**Step 1: Write test and implementation**

In `codedb-core/src/codedb.rs`:

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter};
use tantivy_sqlite::TantivyVTab;

use crate::schema::init_schema;

pub struct CodeDB {
    root: PathBuf,
    conn: Connection,
    code_index: Index,
    diff_index: Index,
    code_reader: IndexReader,
    diff_reader: IndexReader,
    // Field handles for indexing
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
        // Create directory structure
        std::fs::create_dir_all(root.join("tantivy/code_search"))?;
        std::fs::create_dir_all(root.join("tantivy/diff_search"))?;
        std::fs::create_dir_all(root.join("repos"))?;

        // Open SQLite
        let db_path = root.join("db.sqlite");
        let conn = Connection::open(&db_path)
            .context("Failed to open SQLite database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        init_schema(&conn)?;

        // Open or create Tantivy indexes
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

        // Register Tantivy virtual tables
        TantivyVTab::builder()
            .index(code_index.clone())
            .reader(code_reader.clone())
            .search_fields(vec![code_content_field])
            .column("blob_id", code_blob_id_field)
            .score_column("score")
            .snippet_column("snippet", code_content_field)
            .register(&conn, "code_search")?;

        TantivyVTab::builder()
            .index(diff_index.clone())
            .reader(diff_reader.clone())
            .search_fields(vec![diff_content_field])
            .column("diff_id", diff_id_field)
            .score_column("score")
            .snippet_column("snippet", diff_content_field)
            .register(&conn, "diff_search")?;

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
        let mut stmt = db.conn().prepare(
            "SELECT blob_id FROM code_search('test')"
        ).unwrap();
        let results: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
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
}
```

**Step 2: Update lib.rs**

```rust
pub mod schema;
pub mod codedb;

pub use codedb::CodeDB;
```

**Step 3: Run tests**

Run: `cargo test -p codedb-core`
Expected: 4 tests pass

**Step 4: Commit**

```bash
git add codedb-core/src/
git commit -m "feat: add CodeDB struct with open/create, Tantivy indexes, vtab registration"
```

---

### Task 4: Git clone and fetch helpers

Functions to bare-clone a repo from URL, or fetch updates in an existing bare repo.

**Files:**
- Create: `codedb-core/src/git_ops.rs`
- Modify: `codedb-core/src/lib.rs`

**Step 1: Implement clone/fetch/open helpers**

```rust
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

/// Derive a local path for a repo from its URL.
/// "https://github.com/ylow/SFrameRust/" → "github.com/ylow/SFrameRust.git"
pub fn repo_dir_from_url(url: &str) -> Result<String> {
    // Strip scheme
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("git://"))
        .unwrap_or(url);

    // Strip trailing slashes and .git suffix, then re-add .git
    let cleaned = stripped.trim_end_matches('/').trim_end_matches(".git");
    if cleaned.is_empty() {
        anyhow::bail!("Invalid repo URL: {}", url);
    }
    Ok(format!("{}.git", cleaned))
}

/// Clone a bare repo, or fetch if it already exists.
/// Returns the opened gix::Repository.
pub fn clone_or_fetch(url: &str, repo_path: &Path) -> Result<gix::Repository> {
    if repo_path.exists() {
        // Open and fetch
        let repo = gix::open(repo_path)
            .context("Failed to open existing repo")?;
        fetch(&repo)?;
        Ok(repo)
    } else {
        // Clone bare
        clone_bare(url, repo_path)
    }
}

fn clone_bare(url: &str, path: &Path) -> Result<gix::Repository> {
    std::fs::create_dir_all(path)?;
    let mut prepare = gix::prepare_clone_bare(url, path)
        .context("Failed to prepare clone")?;

    let (repo, _outcome) = prepare
        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
        .context("Failed to fetch during clone")?;

    Ok(repo)
}

fn fetch(repo: &gix::Repository) -> Result<()> {
    let remote = repo
        .find_remote("origin")
        .context("Failed to find remote 'origin'")?;

    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .context("Failed to connect to remote")?;

    connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .context("Failed to prepare fetch")?
        .receive(gix::progress::Discard, &AtomicBool::new(false))
        .context("Failed to receive fetch data")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_dir_from_url() {
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust/").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust.git").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
        assert_eq!(
            repo_dir_from_url("https://github.com/ylow/SFrameRust").unwrap(),
            "github.com/ylow/SFrameRust.git"
        );
    }
}
```

**Step 2: Update lib.rs**

```rust
pub mod schema;
pub mod codedb;
pub mod git_ops;

pub use codedb::CodeDB;
```

**Step 3: Run tests**

Run: `cargo test -p codedb-core`
Expected: URL parsing tests pass. Clone/fetch not unit-tested (requires network).

**Step 4: Commit**

```bash
git add codedb-core/src/
git commit -m "feat: add git clone/fetch helpers using gix"
```

---

### Task 5: Language detection from file path

Simple extension-based language detection.

**Files:**
- Create: `codedb-core/src/language.rs`
- Modify: `codedb-core/src/lib.rs`

**Step 1: Implement and test**

```rust
/// Detect programming language from file path extension.
pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "jsx" => Some("jsx"),
        "java" => Some("java"),
        "c" => Some("c"),
        "h" => Some("c"),
        "cpp" | "cc" | "cxx" => Some("cpp"),
        "hpp" | "hxx" | "hh" => Some("cpp"),
        "go" => Some("go"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "scala" => Some("scala"),
        "cs" => Some("csharp"),
        "sh" | "bash" => Some("shell"),
        "sql" => Some("sql"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "xml" => Some("xml"),
        "md" | "markdown" => Some("markdown"),
        "r" => Some("r"),
        "lua" => Some("lua"),
        "zig" => Some("zig"),
        "ex" | "exs" => Some("elixir"),
        "erl" | "hrl" => Some("erlang"),
        "hs" => Some("haskell"),
        "ml" | "mli" => Some("ocaml"),
        "pl" | "pm" => Some("perl"),
        "proto" => Some("protobuf"),
        "dart" => Some("dart"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("src/main.rs"), Some("rust"));
        assert_eq!(detect_language("lib.py"), Some("python"));
        assert_eq!(detect_language("Cargo.toml"), Some("toml"));
        assert_eq!(detect_language("README.md"), Some("markdown"));
        assert_eq!(detect_language("Makefile"), None);
        assert_eq!(detect_language("foo.bar.rs"), Some("rust"));
    }
}
```

**Step 2: Update lib.rs**

Add `pub mod language;`

**Step 3: Run tests, commit**

```bash
cargo test -p codedb-core
git add codedb-core/src/
git commit -m "feat: add extension-based language detection"
```

---

### Task 6: Indexing pipeline — index_repo

The core indexing logic. This is the largest task. It wires together git_ops,
schema, Tantivy indexing, and the CodeDB struct.

**Files:**
- Create: `codedb-core/src/indexer.rs`
- Modify: `codedb-core/src/codedb.rs` (add `index_repo` method)
- Modify: `codedb-core/src/lib.rs`

**Step 1: Implement the indexer**

`codedb-core/src/indexer.rs` — this file contains the core indexing logic.
It should:

1. Call `clone_or_fetch` to get the gix repo
2. Upsert into `repos` table
3. Read all refs from gix
4. For each ref, walk commits from tip to last-indexed commit
5. For each new commit: insert commit, parents, compute diff, insert diffs,
   index new blobs in Tantivy, index diff text in Tantivy
6. Build file_revs for each ref tip
7. Update refs table

Key implementation notes:
- Use `INSERT OR IGNORE` for blobs (content-addressable dedup)
- Use `INSERT OR IGNORE` for commits (may be reachable from multiple refs)
- Use a HashSet of known commit hashes loaded from DB to know when to stop walking
- For the initial index of a ref, walk the full tree at the tip for file_revs
- For incremental updates, copy old file_revs and apply diffs
- Wrap everything in a SQLite transaction for atomicity
- Use `gix::traverse::tree::Recorder` to walk trees
- Use `tree.changes()?.for_each_to_obtain_tree()` for diffs
- Generate unified diff text by reading old and new blob content
- Skip binary files (blobs that aren't valid UTF-8) for Tantivy indexing

The implementation should be straightforward but long. Write it as a single
`pub fn index_repo(db: &mut CodeDB, url: &str) -> Result<()>` function,
extracting helpers as needed.

**Step 2: Add `index_repo` to CodeDB**

In `codedb.rs`:
```rust
pub fn index_repo(&mut self, url: &str) -> Result<()> {
    crate::indexer::index_repo(self, url)
}
```

**Step 3: Run compilation check**

Run: `cargo build -p codedb-core`
Expected: Compiles

**Step 4: Commit**

```bash
git add codedb-core/src/
git commit -m "feat: implement git indexing pipeline"
```

---

### Task 7: Integration test with real repo

Test the full pipeline against https://github.com/ylow/SFrameRust/

**Files:**
- Create: `codedb-core/tests/integration.rs`

**Step 1: Write integration test**

```rust
use codedb_core::CodeDB;
use tempfile::TempDir;

#[test]
fn test_index_sframerust() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    db.index_repo("https://github.com/ylow/SFrameRust/").unwrap();

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

    // Verify code search works
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
             LIMIT 5"
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!joined.is_empty(), "Should find 'struct' in files via join");
    // All paths should end with .rs (it's a Rust project)
    for (path, _) in &joined {
        assert!(path.ends_with(".rs"), "Expected Rust file, got: {}", path);
    }
}

#[test]
fn test_incremental_update() {
    let tmp = TempDir::new().unwrap();
    let mut db = CodeDB::open(tmp.path()).unwrap();

    // First index
    db.index_repo("https://github.com/ylow/SFrameRust/").unwrap();

    let commit_count_1: i64 = db.conn()
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();

    // Re-index (should be incremental, no new commits)
    db.index_repo("https://github.com/ylow/SFrameRust/").unwrap();

    let commit_count_2: i64 = db.conn()
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();

    assert_eq!(commit_count_1, commit_count_2, "No duplicate commits after re-index");
}
```

**Step 2: Run integration test**

Run: `cargo test -p codedb-core --test integration -- --nocapture`
Expected: Tests pass (requires network access, will take some time for clone)

**Step 3: Commit**

```bash
git add codedb-core/tests/
git commit -m "test: add integration tests for git indexing pipeline"
```

---

### Task 8: CLI implementation

Wire up the CLI binary with clap.

**Files:**
- Modify: `codedb-cli/src/main.rs`

**Step 1: Implement CLI**

```rust
use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};
use codedb_core::CodeDB;

#[derive(Parser)]
#[command(name = "codedb", about = "Code indexing and search")]
struct Cli {
    #[arg(long, default_value = "~/.codedb")]
    root: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Clone and index a git repository
    Index {
        /// Repository URL
        url: String,
    },
    /// Search indexed code
    Search {
        /// Search query
        query: String,
    },
    /// Run raw SQL query
    Sql {
        /// SQL query string
        query: String,
    },
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = expand_tilde(&cli.root);

    match cli.command {
        Commands::Index { url } => {
            let mut db = CodeDB::open(&root)?;
            println!("Indexing {}...", url);
            db.index_repo(&url)?;
            println!("Done.");
        }
        Commands::Search { query } => {
            let db = CodeDB::open(&root)?;
            let mut stmt = db.conn().prepare(
                "SELECT fr.path, cs.score, cs.snippet
                 FROM code_search(?1) cs
                 JOIN blobs b ON b.id = cs.blob_id
                 JOIN file_revs fr ON fr.blob_id = b.id
                 JOIN refs r ON r.commit_id = fr.commit_id
                 GROUP BY fr.path
                 ORDER BY cs.score DESC
                 LIMIT 20"
            )?;
            let results = stmt.query_map([&query], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in results {
                let (path, score, snippet) = row?;
                println!("{} (score: {:.2})", path, score);
                println!("  {}", snippet);
                println!();
            }
        }
        Commands::Sql { query } => {
            let db = CodeDB::open(&root)?;
            let mut stmt = db.conn().prepare(&query)?;
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap().to_string())
                .collect();
            println!("{}", col_names.join("\t"));

            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let vals: Vec<String> = (0..col_count)
                    .map(|i| {
                        row.get::<_, rusqlite::types::Value>(i)
                            .map(|v| format!("{:?}", v))
                            .unwrap_or_else(|_| "NULL".to_string())
                    })
                    .collect();
                println!("{}", vals.join("\t"));
            }
        }
    }

    Ok(())
}
```

Note: Add `dirs-next = "2"` and `rusqlite` to `codedb-cli/Cargo.toml`:
```toml
[dependencies]
codedb-core = { path = "../codedb-core" }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
dirs-next = "2"
rusqlite = { version = "0.32", features = ["bundled"] }
```

**Step 2: Build and test manually**

Run: `cargo build -p codedb-cli`
Run: `cargo run -p codedb-cli -- --root /tmp/test-codedb index https://github.com/ylow/SFrameRust/`
Run: `cargo run -p codedb-cli -- --root /tmp/test-codedb search "fn"`

**Step 3: Commit**

```bash
git add codedb-cli/
git commit -m "feat: implement codedb CLI with index, search, and sql commands"
```

---

### Task 9: Polish and final verification

**Step 1: Run clippy on all crates**

Run: `cargo clippy --workspace -- -D warnings`
Fix any warnings.

**Step 2: Run all tests**

Run: `cargo test --workspace`

**Step 3: Commit and push**

```bash
git add -A
git commit -m "chore: clippy fixes and polish"
git push
```

---

### Task Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1    | Scaffold codedb-core + codedb-cli | — |
| 2    | SQLite schema initialization | 1 |
| 3    | CodeDB struct with open/create | 2 |
| 4    | Git clone/fetch helpers (gix) | 1 |
| 5    | Language detection | 1 |
| 6    | Indexing pipeline (index_repo) | 3, 4, 5 |
| 7    | Integration test with real repo | 6 |
| 8    | CLI implementation | 6 |
| 9    | Polish + final verification | 7, 8 |

Tasks 2, 4, 5 can be parallelized after Task 1.
Task 6 is the largest and most complex — expect gix API adjustments.
