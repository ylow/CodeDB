#!/usr/bin/env bash
set -euo pipefail

# CodeDB Demo — indexes SFrameRust and runs example queries
#
# Usage:
#   ./demo.sh
#
# Prerequisites:
#   cargo build --release   (or just: cargo build)
#
# This script will:
#   1. Index https://github.com/ylow/SFrameRust/ into /tmp/codedb-demo
#   2. Show database stats
#   3. Run a series of example queries demonstrating CodeDB's capabilities

CODEDB="cargo run -p codedb-cli --release --"
ROOT="/tmp/codedb-demo"
REPO="https://github.com/ylow/SFrameRust/"

# Colors for headers
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

header() {
    echo ""
    echo -e "${BOLD}━━━ $1 ━━━${RESET}"
    echo ""
}

run_sql() {
    echo -e "${DIM}\$ codedb sql \"$1\"${RESET}"
    $CODEDB --root "$ROOT" sql "$1"
    echo ""
}

run_search() {
    echo -e "${DIM}\$ codedb search \"$1\"${RESET}"
    $CODEDB --root "$ROOT" search "$1"
}

# ──────────────────────────────────────────────
# Step 1: Build
# ──────────────────────────────────────────────
header "Building CodeDB"
cargo build -p codedb-cli --release 2>&1 | tail -1

# ──────────────────────────────────────────────
# Step 2: Index the repo
# ──────────────────────────────────────────────
header "Indexing $REPO"
rm -rf "$ROOT"
$CODEDB --root "$ROOT" index "$REPO"

# ──────────────────────────────────────────────
# Step 3: Database stats
# ──────────────────────────────────────────────
header "Database Stats"
run_sql "SELECT
  (SELECT COUNT(*) FROM commits) as commits,
  (SELECT COUNT(*) FROM blobs) as unique_blobs,
  (SELECT COUNT(*) FROM diffs) as diffs,
  (SELECT COUNT(*) FROM refs) as refs,
  (SELECT COUNT(*) FROM file_revs) as files_at_tips"

# ──────────────────────────────────────────────
# Step 4: Language breakdown
# ──────────────────────────────────────────────
header "Language Breakdown"
run_sql "SELECT b.language, COUNT(*) as count
FROM blobs b
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main' AND b.language IS NOT NULL
GROUP BY b.language
ORDER BY count DESC"

# ──────────────────────────────────────────────
# Step 5: Recent commits
# ──────────────────────────────────────────────
header "Recent Commits"
run_sql "SELECT substr(hash, 1, 10) as hash, author, substr(message, 1, 65) as message
FROM commits ORDER BY timestamp DESC LIMIT 10"

# ──────────────────────────────────────────────
# Step 6: Full-text code search
# ──────────────────────────────────────────────
header "Code Search: 'rayon parallel'"
run_sql "SELECT fr.path, round(cs.score, 2) as score
FROM code_search('rayon parallel') cs
JOIN blobs b ON b.id = cs.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
GROUP BY fr.path
ORDER BY cs.score DESC
LIMIT 10"

# ──────────────────────────────────────────────
# Step 7: Search with snippets
# ──────────────────────────────────────────────
header "Search with Snippets: 'FlexType'"
run_search "FlexType"

# ──────────────────────────────────────────────
# Step 8: Diff search — find commits that touched 'streaming'
# ──────────────────────────────────────────────
header "Diff Search: commits that touched 'streaming'"
run_sql "SELECT substr(c.hash, 1, 10) as hash, substr(c.message, 1, 65) as message, round(ds.score, 2) as score
FROM diff_search('streaming') ds
JOIN diffs d ON d.id = ds.diff_id
JOIN commits c ON c.id = d.commit_id
GROUP BY c.hash
ORDER BY ds.score DESC
LIMIT 10"

# ──────────────────────────────────────────────
# Step 9: Filter by file extension
# ──────────────────────────────────────────────
header "Code Search: 'serialize' in .rs files only"
run_sql "SELECT fr.path, round(cs.score, 2) as score
FROM code_search('serialize') cs
JOIN blobs b ON b.id = cs.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main' AND fr.path GLOB '*.rs'
GROUP BY fr.path
ORDER BY cs.score DESC
LIMIT 10"

# ──────────────────────────────────────────────
# Step 10: Incremental re-index
# ──────────────────────────────────────────────
header "Incremental Re-index (should be fast — no new commits)"
time $CODEDB --root "$ROOT" index "$REPO"

run_sql "SELECT COUNT(*) as total_commits FROM commits"

header "Demo complete!"
echo "Data directory: $ROOT"
echo "SQLite database: $ROOT/db.sqlite"
echo ""
echo "Try your own queries:"
echo "  cargo run -p codedb-cli --release -- --root $ROOT search \"your query\""
echo "  cargo run -p codedb-cli --release -- --root $ROOT sql \"SELECT ...\""
