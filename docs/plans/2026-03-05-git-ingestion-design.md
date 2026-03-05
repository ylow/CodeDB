# Git Ingestion Pipeline Design

## Summary

Build the `codedb-core` library crate and `codedb-cli` binary crate that clone
remote git repos (bare), walk their history, and populate the SQLite + Tantivy
indexes defined in DESIGN.md. Supports incremental updates.

## Crate Structure

- `codedb-core` — library with `CodeDB` struct, indexing pipeline, schema setup
- `codedb-cli` — thin CLI binary wrapping `codedb-core`

## Data Directory Layout

```
{root}/
  db.sqlite
  tantivy/code_search/
  tantivy/diff_search/
  repos/{host}/{owner}/{name}.git/
```

## Public API

```rust
impl CodeDB {
    pub fn open(root: &Path) -> Result<Self>;
    pub fn index_repo(&mut self, url: &str) -> Result<()>;
    pub fn conn(&self) -> &Connection;
}
```

## CLI

```
codedb --root DIR index URL
codedb --root DIR search QUERY
codedb --root DIR sql "SELECT ..."
```

## Indexing Pipeline

1. Clone bare or fetch existing
2. Walk refs, commits, diffs via gix
3. Insert commits, blobs, diffs, file_revs into SQLite
4. Index blob content and diff text in Tantivy
5. Incremental: only process new commits since last indexed tip

## Testing

Integration test against https://github.com/ylow/SFrameRust/
- Verify table population
- Verify text search works
- Verify diff search works
- Verify incremental update works
