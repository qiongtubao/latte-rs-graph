# latte-rs-graph

**Universal code graph library with a swappable backend, designed from
day one for AI agents.**

> 100% local · zero model downloads · zero API keys · single binary CLI
> + a JSON-RPC MCP server exposing every tool to Claude Code / Cursor
> / any MCP-compatible client.

latte-rs-graph builds a queryable knowledge graph of any source tree via
[tree-sitter](https://tree-sitter.github.io/), then exposes structural
analysis back to agents through 13 MCP tools or 11 CLI subcommands.

|               |                                                |
| ------------- | ---------------------------------------------- |
| Languages     | Rust, TypeScript, JavaScript, Python, Go, C, C++ (7 in v1; growable) |
| Storage       | SQLite (bundled) with FTS5 + identifier tokenizer |
| Query         | Cypher subset + graph-augmented grep + BM25 search |
| Identifiers   | camelCase / snake_case splitting baked in |
| Embeddings    | **11-signal algorithmic similarity**, no model required |
| Distribution  | `cargo install latte-rs-graph` — single static binary via `lrg` |

---

## Quick start

```bash
# Build
cargo install latte-rs-graph

# Index a project
lrg build /path/to/project

# Search by identifier (camelCase / snake_case tokens auto-split)
lrg search .latte/graph.db updateCloudClient

# Cypher query
lrg cypher .latte/graph.db \
    'MATCH (a:Function)-[:CALLS]->(b:Function)
     WHERE a.name STARTS WITH "process"
     RETURN a.name, b.name LIMIT 5'

# Find dead code
lrg dead .latte/graph.db

# Blast radius of an edit
lrg blast .latte/graph.db HEAD~3

# Semantic similarity — "what functions look like this one?"
lrg sem-similar .latte/graph.db architecture_overview -n 5
```

Outputs are JSON when `--json` is passed, otherwise human-readable with
`emoji + ascii` headers.

---

## 11 CLI subcommands

| Command                                | What it does |
| -------------------------------------- | ------------ |
| `lrg build <path>`                     | Index a project into `<path>/.latte/graph.db` |
| `lrg stats <db>`                       | Per-kind node / edge counts |
| `lrg search <db> <query>`              | BM25-ranked identifier search (camelCase / snake_case aware) |
| `lrg export <db> --limit N`            | Full graph as JSON |
| `lrg subgraph <db> <node> -d N`        | BFS neighbourhood |
| `lrg grep <db> <root> <pat> --ext rs`  | Graph-augmented grep (definition / usage / test ranking) |
| `lrg complexity <db> <name>`           | 7-metric complexity: cyclomatic / cognitive / loop-depth / alloc-in-loop / linear-scan-in-loop / recursion / unguarded-recursion |
| `lrg dead <db>`                        | Zero-callers, excluding entry points |
| `lrg blast <db> [BASE] -l 3`           | `git diff` → transitive CALLS impact set |
| `lrg arch <db> --aspects hotspots`     | Multi-aspect architecture overview (11 aspects) |
| `lrg cypher <db> "<query>"`            | Cypher v1 subset (MATCH / OPTIONAL MATCH / UNION / WHERE / RETURN / ORDER BY / LIMIT) |
| `lrg sem-similar <db> <name> -n 5`     | Top-N algorithmically similar functions |

---

## 13 MCP tools

Add to `~/.claude/mcp_servers.json` (or equivalent for your client):

```json
{
  "mcpServers": {
    "latte-rs-graph": {
      "command": "/path/to/latte-mcp"
    }
  }
}
```

The `latte-mcp` binary speaks JSON-RPC 2.0 over stdio. Tool names mirror
the CLI surface where possible:

- `index_repository`
- `list_projects`, `index_status`
- `search_graph`, `find_definitions`, `get_subgraph`
- `query_graph` (Cypher)
- `trace_path`
- `get_architecture`
- `search_code`
- `dead_code`
- `blast_radius`
- `complexity`

---

## Architecture

```
src/
├── lib.rs              # public surface
├── types.rs            # Node / Edge / CypherRows / ComplexityMetrics ...
├── traits.rs           # GraphProvider trait — swappable engines
├── error.rs
├── engine/
│   ├── tree_sitter.rs  # default engine — parse → emit nodes/edges
│   ├── metrics.rs      # AST-walking complexity analyzer
│   └── semantic/       # 11-signal algorithmic embeddings
├── storage/
│   ├── sqlite.rs       # SQLite + FTS5 + recursive CTE for blast radius
│   └── memory.rs       # in-memory mirror for tests
└── query/
    ├── grep.rs         # graph-augmented code search
    ├── traversal.rs    # BFS, impact analysis
    ├── subgraph.rs     # extract_subgraph
    └── cypher/         # full Cypher v1 subset (lexer / parser / planner / executor)
```

`GraphProvider` is a single async trait that abstracts the engine. New
backends (a remote service, a Python sidecar, a different parser) just
implement it.

---

## 11-signal semantic similarity (no model!)

Every callable indexed by `lrg build` gets a 768-dim embedding stored in
`node.extra`. After indexing, the top-K most similar pairs receive
`SemanticallyRelated` edges at threshold 0.75 (per-node cap 10).

The signals (weighted blend, defaults sum to 1.0):

| weight | signal                                  | source                              |
| ------ | --------------------------------------- | ----------------------------------- |
| 0.30   | TF-IDF cosine over tokenized names     | tree-sitter identifiers + stopwords |
| 0.30   | Random Indexing post-diffusion          | 768-dim sparse vec + label-prop     |
| 0.10   | parameter-type signature Jaccard        | tree-sitter                         |
| 0.10   | 25-dim AST profile cosine               | tree-sitter                         |
| 0.05   | identifier-touch (data flow) Jaccard    | tree-sitter                         |
| 0.05   | module proximity (file-path prefix)     | file paths                         |
| 0.05   | 64-shingle MinHash Jaccard              | shingle hash                       |
| 0.05   | structural boost (exported + async + non-test) | node flags             |

Zero external embedding, zero model download, deterministic output.
cbm ships a bundled 40 K-token nomic-embed-code binary to do roughly the
same — latte-rs-graph achieves it with 100% algorithm.

---

## License

MIT (same as the wider Latte stack).
