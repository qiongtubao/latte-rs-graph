# Changelog

All notable changes to **latte-rs-graph** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

---

## [0.1.0] — 2026-07-19

The first public release. Single binary CLI (`lrg`) + JSON-RPC MCP server
(`latte-mcp`).

### Added

#### Indexing pipeline
- **Tree-sitter parser**: AST-based indexing for Rust, TypeScript,
  JavaScript, Python, Go, C, C++ (7 languages in v1).
- **Reference resolution**: forward / method / path-import target binding
  with file-scoped name disambiguation.
- **Incremental updates**: `update_files` re-parses only the changed files,
  prunes inbound edges from other files.
- **Language-level modifier detection**: `pub` / `pub(crate)` / `async` /
  `static` / `abstract` / `export` / `default` flag detection during parse
  (previously hardcoded `false`).

#### Storage
- **SQLite with FTS5**: identifier-aware tokenizer splits
  camelCase / snake_case identifiers into BM5-indexed tokens.
- **In-memory mirror**: `MemoryStorage` for tests and small graphs.
- **Row-level FTS sync**: `nodes_fts` virtual table stays consistent
  through `upsert_node` / `upsert_nodes_batch` / `delete_nodes_for_file`
  / `clear_all` / `delete_edges_involving`.
- **File-record population**: `build()` writes `files` table (previously
  only `update_files` did — first build had an empty file list).

#### Query
- **`MATCH` patterns**: `(n:Label)`, `(a:Label)-[:EDGE]->(b:Label)`, and
  variable-length `(a)-[:EDGE*N..M]->(b)`.
- **`OPTIONAL MATCH`**: left-outer-join semantics with `NULL` bindings on
  unmatched side.
- **`UNION` / `UNION ALL`**: depth-2 chain; default `UNION` dedupes by
  serialized row content; `UNION ALL` keeps duplicates.
- **`WHERE` filters**: `=`, `!=`, `<`, `<=`, `>`, `>=`, `IN`, `STARTS WITH`,
  `CONTAINS`.
- **`RETURN` projections**: `var`, `var.prop`, `COUNT(*)`, `AS alias`.
- **`ORDER BY` / `LIMIT`**: per-side, position-based.
- **Cypher reject list** (parser-side): `WITH`, `UNWIND`, multi-clause
  `MATCH` after `OPTIONAL`, 3+ way `UNION`, `WHERE` after `UNION`.

#### Analysis (read-only)
- **`graph-augmented code search`** (`lrg grep`): literal whitespace-AND
  substring matches, deduped to the smallest enclosing
  Function/Method/Class node, ranked Definition > Usage > Test with
  in-degree tie-breaks.
- **`complexity`** (`lrg complexity`): per-function AST walk.
  - `cyclomatic` (1 + branches)
  - `cognitive` (1 + depth at each branch)
  - `max_loop_depth`
  - `alloc_in_loop` (Vec::new / Box::new / .collect / vec! inside loops)
  - `linear_scan_in_loop` (.find / .contains / .indexOf / .binary_search
    inside loops)
  - `is_recursive` (self-call)
  - `unguarded_recursion` (recursive with no if-return guard before call)
- **`dead_code`**: SQL NOT-IN subselect plus the entry-point heuristic
  (name in `{main, index, __init__}` OR `is_exported=1` OR
  `visibility ∈ {public, pub}` OR file matches `bin/` / `main.*` /
  `index.*`).
- **`blast_radius`**: git-diff → seed function set → recursive-CTE
  BFS over CALLS edges (inbound / outbound / both) within configurable
  depth, with `git -C` automatically derived from the db location.
- **`architecture_overview`**: 11-aspect multi-section report.
  - `overview` — counts (nodes, edges, files, routes, languages,
    entry points, dead, call edges)
  - `structure` / `packages` — top-tier directory rollup
  - `dependencies` — cross-file edge top-N
  - `routes` — every `kind=route` node
  - `languages` — per-language node / edge / file counts
  - `entry_points` — inverse of dead (entry candidates)
  - `hotspots` — top-N Function/Method by inbound CALLS degree
  - `boundaries` — module fan-in vs fan-out ratio
  - `layers` — BFS layer histogram from entry points
  - `file_tree` — ASCII tree
  - `clusters` / `cycles` — reserved (Phase 6: future Leiden)

#### 11-signal algorithmic semantic similarity (no model!)
- **Per-function embedding** computed during `build()` and stored in
  `node.extra`.
- **Edge emission**: top-K similar pairs receive `SemanticallyRelated`
  edges at threshold 0.75 (per-node cap 10).
- **CLI**: `lrg sem-similar <db> <name> -n 5 [--json]` reads any
  callable's stored embedding and ranks against the full graph.
- **No external embedding**: 100% algorithm, deterministic, no model
  download, no API key.

#### Distribution
- **`lrg` CLI binary** with 11 subcommands (`build`, `stats`, `search`,
  `export`, `subgraph`, `grep`, `complexity`, `dead`, `blast`, `arch`,
  `cypher`, `sem-similar`).
- **`latte-mcp` JSON-RPC MCP server**: 13 tools, no extra crate
  dependency. ~700 LoC hand-rolled. Standard Claude Code / Cursor
  config (`~/.claude/mcp_servers.json`) wires it directly.

### Tests
92 integration tests across `cargo test --lib` + `cargo test --bin latte-mcp`:
- 38 storage / engine baseline tests
- 11 cypher executor + parser tests (4 NEW)
- 7 semantics unit tests
- 6 architecture-overview tests
- 5 metrics tests (cyclomatic / cognitive / loop depth / alloc / recursion)
- 7 grep tests
- 10 trace coverage tests
- 3 MCP server integration tests
- remaining = Phase 1 / Phase 2 / Phase 3 baseline

---

[0.1.0]: #010--2026-07-19
