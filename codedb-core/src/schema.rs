use rusqlite::Connection;

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA_SQL)?;
    // Migration: add type-info columns if upgrading from an older schema
    migrate_add_type_info(conn)?;
    Ok(())
}

fn migrate_add_type_info(conn: &Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('symbols') WHERE name='signature'",
        [],
        |r| r.get(0),
    )?;
    if !has_col {
        conn.execute_batch(
            "ALTER TABLE symbols ADD COLUMN signature TEXT;
             ALTER TABLE symbols ADD COLUMN return_type TEXT;
             ALTER TABLE symbols ADD COLUMN params TEXT;
             UPDATE blobs SET parsed = 0 WHERE language IS NOT NULL;",
        )?;
    }
    Ok(())
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS repos (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL
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

CREATE TABLE IF NOT EXISTS refs (
    id        INTEGER PRIMARY KEY,
    repo_id   INTEGER NOT NULL REFERENCES repos(id),
    name      TEXT NOT NULL,
    commit_id INTEGER NOT NULL REFERENCES commits(id),
    UNIQUE(repo_id, name)
);

CREATE TABLE IF NOT EXISTS blobs (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    language     TEXT,
    parsed       INTEGER NOT NULL DEFAULT 0
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
    id          INTEGER PRIMARY KEY,
    blob_id     INTEGER NOT NULL REFERENCES blobs(id),
    parent_id   INTEGER REFERENCES symbols(id),
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    line        INTEGER NOT NULL,
    col         INTEGER NOT NULL,
    end_line    INTEGER,
    end_col     INTEGER,
    signature   TEXT,
    return_type TEXT,
    params      TEXT
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
        conn.execute(
            "INSERT INTO repos (name, path) VALUES ('test', '/tmp/test')",
            [],
        )
        .unwrap();
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
