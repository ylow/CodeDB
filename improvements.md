# Potential Improvements

Sourcegraph query language features not currently supported, ranked by
feasibility and value.

## Easy wins (done)

### 1. `@revision` syntax — DONE

`repo:foo@develop` is now split into `repo:foo` + `rev:develop`.

### 2. Negation for more filters — DONE

`-repo:`, `-lang:`, `-author:`, `-message:` all supported alongside `-file:`.

### 3. `patterntype:literal` / `patterntype:keyword` — DONE

Accepted as no-ops (already the default behavior). `patterntype:regexp` and
`patterntype:structural` produce a clear error message.

## Moderate effort, high value

### 4. Regex in code/diff search

Support `/pattern/` syntax for regex matching in full-text search.

Tantivy already has a regex mode in `query_builder.rs`. The work is:
- Detect `/pattern/` syntax in the parser
- Pass a mode flag through to the Tantivy virtual table query string
  (e.g., a `regex:` prefix convention in the vtab query)
- Does NOT help SQL-level matching (symbol names, commit messages), but
  those are the less common case

This is the highest-value moderate-effort feature — regex is the most common
Sourcegraph capability that users would expect to work.

### 5. `OR` for search terms

Support `foo OR bar` to match either term.

Tantivy naturally handles multi-term queries with OR semantics. For SQL-level
queries (symbol, commit), generating `(col LIKE '%foo%' OR col LIKE '%bar%')`
is straightforward. The real work is parsing — need to distinguish `foo OR bar`
from `foo bar` (implicit AND). Not trivial but not huge.

## Probably not worth it

### 6. Full boolean expressions with parentheses

`(foo OR bar) AND baz`, `NOT pattern`, arbitrary nesting.

Requires a recursive descent parser, an AST representation, and recursive SQL
generation. Significant complexity for a feature rarely needed at single-repo
scale. Raw SQL is available as an escape hatch.

### 7. Structural search

Sourcegraph's Comby-powered `type:structural` matching (e.g.,
`fmt.Sprintf(:[args])`). Would require integrating Comby or building an
equivalent. Large effort. The `calls:`, `calledby:`, and `returns:` filters
already cover the most common use cases that structural search addresses.

### 8. Repository-level filters

`fork:`, `archived:`, `visibility:`, `repogroup:`, `repo:has.file()`,
`repo:has.path()`, `file:has.owner()`.

These are designed for filtering across a large corpus of repositories on a
hosted Sourcegraph instance. They don't apply when searching one or a few
locally-indexed repos.

### 9. `timeout:`, `stable:`

Query execution controls. Low value — queries run locally and finish fast.
Deterministic ordering is trivially achievable with `ORDER BY` in raw SQL.
