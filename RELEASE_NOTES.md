# Release Notes — 0.1.0

**latte-rs-graph 0.1.0** — initial public release
2026-07-19

This is the **first** public release of latte-rs-graph. It bundles 9
months of design + implementation that drove ten separate capability
phases, each landing as an isolated commit with its own tests.

---

## What's in 0.1.0

A code-graph library + binaries for AI agents:

- **`lrg`** — single CLI binary with 11 subcommands
- **`latte-mcp`** — JSON-RPC MCP server with 13 tools

Three pillars, each on par with the corresponding CBM tool:

1. **Indexing** — tree-sitter AST walks produce a `nodes`/`edges`/`files`
   schema in SQLite (bundled).
2. **Query** — Cypher v1 subset (`MATCH` / `OPTIONAL MATCH` / `UNION` /
   `WHERE` / `RETURN` / `ORDER BY` / `LIMIT`) plus graph-augmented grep
   plus FTS5 search with identifier tokenizer.
3. **Analysis** — 11-signal algorithmic similarity (no model), blast
   radius, dead code, complexity, multi-aspect architecture overview.

The 11-signal similarity is the **brand differentiator**: CBM ships a
bundled 40 K-token nomic-embed-code binary; latte-rs-graph achieves the
same shape with 100% algorithm and zero model download.

---

## Breaking changes since last version

None — first public release.

## Compatibility matrix

| Rust toolchain | tested |
| -------------- | ------ |
| 1.96.0 stable  | yes (`rustc --version` on the dev machine) |

**Edition**: 2024.

**Platform support**: any `cargo` target (Linux x86_64 / ARM64 / macOS
x86_64 + arm64 / Windows). SQLite is bundled via `rusqlite`'s `bundled`
feature flag — no system SQLite needed.

---

## What's *not* in 0.1.0

These are explicitly out of scope for v1, deferred to later:

- Leiden / Louvain community detection (only the placeholder enum
  variant in `get_architecture` is exposed; `lrg arch --aspects clusters`
  prints a friendly "Phase 6" message).
- Strongly-connected-component cycles (same as above).
- Code snippet retrieval by qualified name.
- Architecture Decision Records (ADRs).
- Index coverage report.
- Trace ingestion.
- HTTP / gRPC / GraphQL / tRPC cross-service edges.
- Multi-repo (`CROSS_*`) edges.

If you need any of these, file an issue — they're prioritized by user
demand.

---

## Upgrade notes for early adopters

There are no earlier versions; this *is* the first one.

If you upgrade from a development build (`git clone` of the repo at
HEAD before this tag), note:

- The `Phase 6b` fix to `is_exported` (commit `d8b2f08`) makes
  `entry_points_count` non-zero for the first time. **Re-run `lrg
  build`** on any pre-existing graph to populate the modifiers.
- The `build()` SQL path now writes `FileRecord` rows (previously only
  `update_files` did). Re-building is required for `lrg grep` to see
  candidate files on a fresh index.

---

## Verification

- `cargo build --release` — clean.
- `cargo test --lib` — 89 passed, 0 failed.
- `cargo test --bin latte-mcp` — 3 passed, 0 failed.
- `cargo clippy --lib --no-deps` — no warnings on files added in 0.1.0;
  pre-existing warnings in `src/query/traversal.rs` and `src/types.rs`
  (clippy `sort_by_key` suggestions) preserved.

## Smoke

A 6-function Rust project (`/tmp/fts-smoke`) indexes cleanly. Live examples:

```bash
$ lrg build /tmp/fts-smoke
✅ Build complete in 0.03s
   Nodes created:  7  (1 file + 6 functions)

$ lrg cypher .latte/graph.db \
      'MATCH (n:Function) WHERE n.name STARTS WITH "parse" RETURN n.name'
parseUserInput
parse_user_input

$ lrg sem-similar .latte/graph.db architecture_overview -n 3
🔮 most-similar to src::storage::sqliters::architecture_overview (src/storage/sqlite.rs)
   1. [0.979] function src::storage::memoryrs::architecture_overview (src/storage/memory.rs:354)
   2. [0.895] function src::bin::mainrs::render_arch_report (src/bin/main.rs:583)
   3. [0.821] function src::typesrs::parse_list (src/types.rs:707)
```

Output of the latte project itself:

```
$ lrg build .                            # 21 files
   Files scanned: 21
   Nodes created: 372 (function × 265 + struct × 54 + ...)

$ sqlite3 .latte/graph.db "SELECT count(*) FROM edges WHERE kind='semantically_related'"
104
```
