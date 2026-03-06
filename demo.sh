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
#   3. Run a series of queries demonstrating Sourcegraph-style search syntax

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

run_search() {
    echo -e "${DIM}\$ codedb search \"$1\"${RESET}"
    $CODEDB --root "$ROOT" search "$1"
    echo ""
}

run_search_sql() {
    echo -e "${DIM}\$ codedb search --sql \"$1\"${RESET}"
    $CODEDB --root "$ROOT" search --sql "$1"
    echo ""
}

run_sql() {
    echo -e "${DIM}\$ codedb sql \"$1\"${RESET}"
    $CODEDB --root "$ROOT" sql "$1"
    echo ""
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
  (SELECT COUNT(*) FROM symbols) as symbols,
  (SELECT COUNT(*) FROM symbol_refs) as call_refs,
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

# ══════════════════════════════════════════════
# Sourcegraph-style queries start here
# ══════════════════════════════════════════════

# ──────────────────────────────────────────────
# Step 5: Basic code search with snippets
# ──────────────────────────────────────────────
header "Code Search: FlexType"
run_search "FlexType count:5"

# ──────────────────────────────────────────────
# Step 6: Filtered code search
# ──────────────────────────────────────────────
header "Code Search: 'serialize' in Rust source files (no tests)"
run_search "lang:rust file:*.rs -file:test serialize"

# ──────────────────────────────────────────────
# Step 7: Show generated SQL (--sql flag)
# ──────────────────────────────────────────────
header "Show Generated SQL"
run_search_sql "lang:rust type:symbol SFrame"

# ──────────────────────────────────────────────
# Step 8: Symbol search — find all structs named SFrame
# ──────────────────────────────────────────────
header "Symbol Search: structs matching 'SFrame'"
run_search "type:symbol select:symbol.struct lang:rust SFrame"

# ──────────────────────────────────────────────
# Step 9: Symbol search — all functions in the csv_parser module
# ──────────────────────────────────────────────
header "Symbol Search: functions in csv_parser"
run_search "type:symbol select:symbol.function file:csv_parser"

# ──────────────────────────────────────────────
# Step 10: Who calls groupby()?
# ──────────────────────────────────────────────
header "Cross-reference: who calls groupby()?"
run_search "calls:groupby count:15"

# ──────────────────────────────────────────────
# Step 11: What does groupby() call?
# ──────────────────────────────────────────────
header "Cross-reference: what does groupby() call?"
run_search "calledby:groupby"

# ──────────────────────────────────────────────
# Step 12: Which functions use parallel iteration?
# ──────────────────────────────────────────────
header "Cross-reference: functions that call par_iter() (rayon parallel)"
run_search "calls:par_iter"

# ──────────────────────────────────────────────
# Step 13: Functions returning BatchIterator
# ──────────────────────────────────────────────
header "Type Info: functions returning BatchIterator"
run_search "returns:BatchIterator"

# ──────────────────────────────────────────────
# Step 14: Functions returning SFrame (in query module)
# ──────────────────────────────────────────────
header "Type Info: functions returning SFrame in query module"
run_search "returns:SFrame file:sframe-query count:10"

# ──────────────────────────────────────────────
# Step 15: Functions taking SFrame parameters (raw SQL)
# ──────────────────────────────────────────────
header "Type Info: functions with SFrame parameters"
run_sql "SELECT DISTINCT fr.path || ':' || s.line AS location, s.params
FROM symbols s
JOIN blobs b ON b.id = s.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE s.params LIKE '%SFrame%'
  AND s.kind = 'function'
  AND r.name = 'refs/heads/main'
ORDER BY fr.path, s.line
LIMIT 10"

# ──────────────────────────────────────────────
# Step 15: Most called functions (excluding builtins)
# ──────────────────────────────────────────────
header "Most called functions (domain-specific)"
run_sql "SELECT sr.ref_name AS function, COUNT(*) AS calls
FROM symbol_refs sr
JOIN blobs b ON b.id = sr.blob_id
JOIN file_revs fr ON fr.blob_id = b.id
JOIN refs r ON r.commit_id = fr.commit_id
WHERE r.name = 'refs/heads/main'
  AND sr.kind = 'call'
  AND sr.ref_name NOT IN (
    'new','unwrap','assert_eq','Ok','len','vec','clone','iter',
    'push','collect','map','format','assert','to_string','expect',
    'get','into','from','Some','None','Err','println','is_empty',
    'write_u64','write_u8','read_u64','read_u8','enumerate','insert'
  )
GROUP BY sr.ref_name
ORDER BY calls DESC
LIMIT 15"

# ──────────────────────────────────────────────
# Step 17: Diff search — commits that touched streaming
# ──────────────────────────────────────────────
header "Diff Search: commits that touched 'streaming' in Rust files"
run_search "type:diff file:*.rs streaming count:5"

# ──────────────────────────────────────────────
# Step 18: Commit search — recent refactors
# ──────────────────────────────────────────────
header "Commit Search: refactoring commits"
run_search "type:commit refactor count:5"

# ──────────────────────────────────────────────
# Step 19: Commit search — by author
# ──────────────────────────────────────────────
header "Commit Search: commits by author with 'parallel' in message"
run_search "type:commit author:Yucheng parallel count:5"

# ──────────────────────────────────────────────
# Step 20: Incremental re-index
# ──────────────────────────────────────────────
header "Incremental Re-index (should be fast — no new commits)"
time $CODEDB --root "$ROOT" index "$REPO"
echo ""

header "Demo complete!"
echo "Data directory: $ROOT"
echo "SQLite database: $ROOT/db.sqlite"
echo ""
echo "Try your own queries:"
echo "  $CODEDB --root $ROOT search \"FlexType\""
echo "  $CODEDB --root $ROOT search \"type:symbol lang:rust SFrame\""
echo "  $CODEDB --root $ROOT search \"returns:BatchIterator\""
echo "  $CODEDB --root $ROOT search \"calls:groupby\""
echo "  $CODEDB --root $ROOT search \"type:diff streaming\""
echo "  $CODEDB --root $ROOT search \"type:commit author:Yucheng parallel\""
echo "  $CODEDB --root $ROOT search --sql \"lang:rust file:*.rs serialize\""
echo "  $CODEDB --root $ROOT sql \"SELECT ...\""
