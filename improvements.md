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

## Moderate effort, high value (done)

### 4. Regex in code/diff search — DONE

`/pattern/` syntax and `patterntype:regexp` supported for code and diff search.
Passes regex mode to the Tantivy virtual table. Errors clearly for symbol/commit
search where SQL-level regex isn't available.

### 5. `OR` for search terms — DONE

`foo OR bar` splits search terms into OR groups. For code/diff search, passed
through to Tantivy which handles OR natively. For symbol/commit search,
generates `(col LIKE '%foo%' OR col LIKE '%bar%')` SQL.
Regex + OR combination is rejected at parse time.

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
