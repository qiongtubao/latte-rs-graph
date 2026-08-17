//! `latte-mcp` — JSON-RPC 2.0 over stdio that exposes the `latte-rs-graph`
//! engine to Model Context Protocol (MCP) clients (Claude Code, etc.).
//!
//! The protocol implementation is hand-rolled (≈200 LoC of dispatch logic) —
//! no extra crate dependency is needed because MCP's wire format is just
//! newline-delimited JSON-RPC 2.0.
//!
//! Three RPC methods are supported:
//!   * `initialize`         — handshake, returns serverInfo + capabilities.
//!   * `tools/list`         — advertises the 13 tool definitions below.
//!   * `tools/call`         — dispatches to a per-tool async handler.
//!
//! Notifications (no `id`) are silently dropped. Unknown methods return a
//! JSON-RPC error with code `-32601`. Stdout carries only JSON-RPC responses
//! (newline-delimited); diagnostics go to stderr.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use latte_rs_graph::engine::TreeSitterEngine;
use latte_rs_graph::error::GraphError;
use latte_rs_graph::storage::SqliteStorage;
use latte_rs_graph::traits::GraphProvider;
use latte_rs_graph::types::{
    ArchitectureAspect, ArchitectureRequest, BlastDirection, BuildOptions, SearchCodeMode,
    SearchCodeRequest, SearchOptions,
};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("mcp: stdin read error: {e}");
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("mcp: invalid json: {e}");
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications carry no `id` — drop them (e.g. `notifications/initialized`).
        if id.is_none() {
            continue;
        }

        let response = match method {
            "initialize" => handle_initialize(),
            "tools/list" => handle_tools_list(),
            "tools/call" => handle_tools_call(params).await,
            other => Err(format!("Method not found: {other}")),
        };

        let reply = match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": message},
            }),
        };

        writeln!(stdout, "{}", reply)?;
        stdout.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Method handlers
// ---------------------------------------------------------------------------

fn handle_initialize() -> Result<Value, String> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "latte-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {"tools": {}},
    }))
}

fn handle_tools_list() -> Result<Value, String> {
    Ok(json!({"tools": tools_definitions()}))
}

async fn handle_tools_call(params: Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `name` in tool call".to_string())?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let result = match name {
        "index_repository" => tool_index_repository(args).await,
        "list_projects" => tool_list_projects(args).await,
        "check_index_coverage" => tool_check_index_coverage(args).await,
        "index_status" => tool_index_status(args).await,
        "search_graph" => tool_search_graph(args).await,
        "find_definitions" => tool_find_definitions(args).await,
        "get_subgraph" => tool_get_subgraph(args).await,
        "query_graph" => tool_query_graph(args).await,
        "trace_path" => tool_trace_path(args).await,
        "get_architecture" => tool_get_architecture(args).await,
        "search_code" => tool_search_code(args).await,
        "dead_code" => tool_dead_code(args).await,
        "blast_radius" => tool_blast_radius(args).await,
        "complexity" => tool_complexity(args).await,
        other => Err(format!("unknown tool: {other}")),
    };
    match result {
        Ok(payload) => Ok(payload),
        Err(message) => Ok(err_content(message)),
    }
}

// ---------------------------------------------------------------------------
// MCP content wrappers
// ---------------------------------------------------------------------------

fn text_content<T: serde::Serialize>(payload: &T) -> Value {
    let body = serde_json::to_string_pretty(payload).unwrap_or_else(|e| {
        format!("{{\"serialization_error\":\"{e}\"}}")
    });
    json!({
        "content": [{"type": "text", "text": body}],
        "isError": false,
    })
}

fn err_content(message: impl Into<String>) -> Value {
    json!({
        "content": [{"type": "text", "text": message.into()}],
        "isError": true,
    })
}

// ---------------------------------------------------------------------------
// Tool definitions (`tools/list` payload)
// ---------------------------------------------------------------------------

fn tools_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "index_repository",
            "description": "Build a fresh code graph for a repository. Creates a SQLite DB at the chosen path (default <repo>/.latte/graph.db).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string", "description": "Absolute path to the project root to index."},
                    "db_path": {"type": "string", "description": "Optional output SQLite path. Defaults to <repo_path>/.latte/graph.db."},
                    "langs": {"type": "array", "items": {"type": "string"}, "description": "Optional language whitelist (e.g. [\"rust\", \"typescript\"])."},
                    "incremental": {"type": "boolean", "description": "When true, re-parse only files whose content changed since the last build (mtime + content hash diff). Falls back to a full build on an empty database."}
                },
                "required": ["repo_path"],
            }
        }),
        json!({
            "name": "list_projects",
            "description": "Enumerate every *.db under a directory (default ~/.cache/latte-rs-graph) and return a summary row per project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": {"type": "string", "description": "Directory to scan for *.db files. Defaults to ~/.cache/latte-rs-graph."}
                }
            }
        }),
        json!({
            "name": "check_index_coverage",
            "description": "Index-coverage honesty report: which files were indexed, skipped (e.g. size limit), or failed to parse. Consult this BEFORE concluding that a symbol, caller, or file does not exist — absence in the graph may mean the file was never indexed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string", "description": "Absolute path to the SQLite graph database."},
                    "path": {"type": "string", "description": "Optional project-relative path prefix to scope the report."}
                },
                "required": ["db"]
            }
        }),
        json!({
            "name": "index_status",
            "description": "Return GraphStats + node summary for an existing database.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string", "description": "Absolute path to the SQLite graph database."}
                },
                "required": ["db"],
            }
        }),
        json!({
            "name": "search_graph",
            "description": "FTS-style keyword/identifier search across nodes. CamelCase / snake_case tokens are split automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "query": {"type": "string", "description": "Keyword or identifier; camelCase / snake_case tokens are split."},
                    "limit": {"type": "integer", "default": 50}
                },
                "required": ["db", "query"],
            }
        }),
        json!({
            "name": "find_definitions",
            "description": "Return every node whose name matches `name` exactly (function/method/class/struct/...).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "name": {"type": "string"}
                },
                "required": ["db", "name"],
            }
        }),
        json!({
            "name": "get_subgraph",
            "description": "Return the neighbors of `node` (in/out) up to `depth` hops.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "node": {"type": "string"},
                    "depth": {"type": "integer", "default": 2}
                },
                "required": ["db", "node"],
            }
        }),
        json!({
            "name": "query_graph",
            "description": "Run a Cypher query against the graph and return flattened columnar rows.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "query": {"type": "string"}
                },
                "required": ["db", "query"],
            }
        }),
        json!({
            "name": "trace_path",
            "description": "BFS callers/callees of a named function along CALLS edges. Returns candidate list if the name is ambiguous.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "function_name": {"type": "string"},
                    "direction": {"type": "string", "enum": ["inbound", "outbound", "both"], "default": "both"},
                    "depth": {"type": "integer", "default": 3}
                },
                "required": ["db", "function_name"],
            }
        }),
        json!({
            "name": "get_architecture",
            "description": "Multi-aspect architecture overview. Aspect names are comma-separated; empty list = the default set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "path": {"type": "string"},
                    "aspects": {"type": "array", "items": {"type": "string"}},
                    "top_n": {"type": "integer", "default": 20}
                },
                "required": ["db"],
            }
        }),
        json!({
            "name": "search_code",
            "description": "Literal substring search across the indexed source tree; each hit is enriched with the smallest enclosing graph node.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "project_root": {"type": "string"},
                    "pattern": {"type": "string"},
                    "extensions": {"type": "array", "items": {"type": "string"}},
                    "path_filter": {"type": "string"},
                    "limit": {"type": "integer", "default": 10}
                },
                "required": ["db", "project_root", "pattern"],
            }
        }),
        json!({
            "name": "dead_code",
            "description": "List callable nodes (function/method/test) with zero callers that are not entry points.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"}
                },
                "required": ["db"],
            }
        }),
        json!({
            "name": "blast_radius",
            "description": "Given a list of changed file paths, return the symbols defined in them plus their transitive CALLS reachability.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "project_root": {"type": "string"},
                    "changed_paths": {"type": "array", "items": {"type": "string"}},
                    "depth": {"type": "integer", "default": 3},
                    "direction": {"type": "string", "enum": ["inbound", "outbound", "both"], "default": "inbound"}
                },
                "required": ["db", "project_root", "changed_paths"],
            }
        }),
        json!({
            "name": "complexity",
            "description": "Look up cyclomatic / cognitive / recursion metrics for a named function.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "db": {"type": "string"},
                    "function_name": {"type": "string"}
                },
                "required": ["db", "function_name"],
            }
        }),
    ]
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing required argument `{key}`"))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn opt_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn opt_u32(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(default)
}

fn opt_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

fn open_readonly(db: &str) -> Result<TreeSitterEngine, String> {
    let path = Path::new(db);
    let storage = SqliteStorage::open_readonly(path).map_err(|e| e.to_string())?;
    Ok(TreeSitterEngine::new(storage))
}


fn direction_arg(args: &Value, key: &str, default: BlastDirection) -> Result<BlastDirection, String> {
    match opt_str(args, key).unwrap_or("").to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "inbound" => Ok(BlastDirection::Inbound),
        "outbound" => Ok(BlastDirection::Outbound),
        "both" => Ok(BlastDirection::Both),
        other => Err(format!("invalid {key}: {other} (expected inbound|outbound|both)")),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

async fn tool_index_repository(args: Value) -> Result<Value, String> {
    let repo_path = require_str(&args, "repo_path")?.to_string();
    let repo = PathBuf::from(&repo_path);
    let db_path = match opt_str(&args, "db_path") {
        Some(s) => PathBuf::from(s),
        None => repo.join(".latte").join("graph.db"),
    };
    let langs: Option<Vec<String>> = args.get("langs").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });

    let storage = SqliteStorage::open(&db_path).map_err(|e| e.to_string())?;
    let engine = TreeSitterEngine::new(storage);

    if opt_bool(&args, "incremental", false) {
        let report = engine.update(&repo).await.map_err(|e| e.to_string())?;
        return Ok(text_content(&json!({
            "db_path": db_path.to_string_lossy(),
            "mode": "incremental",
            "report": report,
        })));
    }

    let opts = BuildOptions {
        project_root: repo_path,
        languages: langs,
        ..BuildOptions::default()
    };
    let report = engine.build(&repo, &opts).await.map_err(|e| e.to_string())?;
    Ok(text_content(&json!({
        "db_path": db_path.to_string_lossy(),
        "mode": "full",
        "report": report,
    })))
}

async fn tool_check_index_coverage(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let path = opt_str(&args, "path").map(|s| s.to_string());
    let engine = open_readonly(db)?;
    let report = engine
        .index_coverage(path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&report))
}

async fn tool_list_projects(args: Value) -> Result<Value, String> {
    let default_root = dirs_cache_root();
    let root = PathBuf::from(opt_str(&args, "root").unwrap_or(&default_root));
    if !root.exists() {
        return Ok(text_content(&json!({
            "root": root.to_string_lossy(),
            "exists": false,
            "projects": [],
        })));
    }
    let mut projects = Vec::new();
    let entries = std::fs::read_dir(&root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("db") {
            continue;
        }
        let summary = match SqliteStorage::open_readonly(&p) {
            Ok(storage) => {
                let stats = storage.stats().ok();
                let path = p.to_string_lossy().to_string();
                let nodes = stats.as_ref().map(|s| s.total_nodes).unwrap_or(0);
                let edges = stats.as_ref().map(|s| s.total_edges).unwrap_or(0);
                let files = stats.as_ref().map(|s| s.total_files).unwrap_or(0);
                Some(json!({"path": path, "nodes": nodes, "edges": edges, "files": files}))
            }
            Err(_) => None,
        };
        if let Some(s) = summary {
            projects.push(s);
        }
    }
    Ok(text_content(&json!({
        "root": root.to_string_lossy(),
        "projects": projects,
    })))
}

fn dirs_cache_root() -> String {
    if let Ok(home) = std::env::var("HOME") {
        return format!("{home}/.cache/latte-rs-graph");
    }
    ".cache/latte-rs-graph".to_string()
}

async fn tool_index_status(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let engine = open_readonly(db)?;
    let stats = engine.stats().await.map_err(|e| e.to_string())?;
    Ok(text_content(&stats))
}

async fn tool_search_graph(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let query = require_str(&args, "query")?;
    let limit = opt_usize(&args, "limit", 50);
    let engine = open_readonly(db)?;
    let hits = engine
        .search(query, &SearchOptions {
            limit,
            ..SearchOptions::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&json!({"query": query, "limit": limit, "hits": hits})))
}

async fn tool_find_definitions(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let name = require_str(&args, "name")?;
    let engine = open_readonly(db)?;
    let defs = engine.find_definitions(name).await.map_err(|e| e.to_string())?;
    Ok(text_content(&json!({"name": name, "matches": defs})))
}

async fn tool_get_subgraph(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let node = require_str(&args, "node")?;
    let depth = opt_u32(&args, "depth", 2);
    let engine = open_readonly(db)?;
    let g = engine.subgraph(node, depth).await.map_err(|e| e.to_string())?;
    Ok(text_content(&g))
}

async fn tool_query_graph(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let query = require_str(&args, "query")?;
    let engine = open_readonly(db)?;
    let rows = engine.cypher(query).await.map_err(|e| e.to_string())?;
    Ok(text_content(&rows))
}

async fn tool_trace_path(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let function_name = require_str(&args, "function_name")?;
    let depth = opt_u32(&args, "depth", 3);
    let engine = open_readonly(db)?;
    let defs = engine
        .find_definitions(function_name)
        .await
        .map_err(|e| e.to_string())?;
    if defs.len() != 1 {
        return Ok(text_content(&json!({
            "found_multiple": true,
            "candidates": defs,
        })));
    }
    let node = defs.into_iter().next().unwrap();
    let direction = direction_arg(&args, "direction", BlastDirection::Both)?;
    let payload = match direction {
        BlastDirection::Inbound => {
            let r = engine.callers(&node.id, depth).await.map_err(|e| e.to_string())?;
            json!({"node": node, "direction": "inbound", "relations": r.relations, "depth": r.depth})
        }
        BlastDirection::Outbound => {
            let r = engine.callees(&node.id, depth).await.map_err(|e| e.to_string())?;
            json!({"node": node, "direction": "outbound", "relations": r.relations, "depth": r.depth})
        }
        BlastDirection::Both => {
            let inb = engine.callers(&node.id, depth).await.map_err(|e| e.to_string())?;
            let out = engine.callees(&node.id, depth).await.map_err(|e| e.to_string())?;
            json!({
                "node": node,
                "direction": "both",
                "callers": inb.relations,
                "callees": out.relations,
            })
        }
    };
    Ok(text_content(&payload))
}

async fn tool_get_architecture(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let engine = open_readonly(db)?;
    let req = ArchitectureRequest {
        path_scope: opt_str(&args, "path").map(|s| s.to_string()),
        aspects: match args.get("aspects").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => {
                let joined: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                ArchitectureAspect::parse_list(&joined.join(","))
            }
            _ => ArchitectureAspect::default_set(),
        },
        top_n: opt_usize(&args, "top_n", 20),
    };
    let report = engine
        .architecture_overview(&req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&report))
}

async fn tool_search_code(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let project_root = require_str(&args, "project_root")?;
    let pattern = require_str(&args, "pattern")?;
    let extensions: Option<Vec<String>> = args
        .get("extensions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    let req = SearchCodeRequest {
        pattern: pattern.to_string(),
        regex: false,
        file_extensions: extensions,
        path_filter: opt_str(&args, "path_filter").map(|s| s.to_string()),
        mode: SearchCodeMode::Compact,
        context_lines: 3,
        limit: opt_usize(&args, "limit", 10),
    };
    let engine = open_readonly(db)?;
    let resp = engine
        .search_code(Path::new(project_root), &req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&resp))
}

async fn tool_dead_code(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let engine = open_readonly(db)?;
    let report = engine.dead_code().await.map_err(|e| e.to_string())?;
    Ok(text_content(&report))
}

async fn tool_blast_radius(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let project_root = require_str(&args, "project_root")?;
    let changed_paths: Vec<String> = match args.get("changed_paths").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        None => return Err("missing required argument `changed_paths`".into()),
    };
    let depth = opt_u32(&args, "depth", 3);
    let direction = direction_arg(&args, "direction", BlastDirection::Inbound)?;
    let _ = project_root; // engine doesn't read it, but we keep the API parity.
    let engine = open_readonly(db)?;
    let report = engine
        .blast_radius(changed_paths, depth, direction, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&report))
}

async fn tool_complexity(args: Value) -> Result<Value, String> {
    let db = require_str(&args, "db")?;
    let function_name = require_str(&args, "function_name")?;
    let engine = open_readonly(db)?;
    let defs = engine
        .find_definitions(function_name)
        .await
        .map_err(|e| e.to_string())?;
    let target = match defs.first() {
        Some(n) => n,
        None => {
            return Ok(err_content(format!(
                "no definition found for `{function_name}`"
            )));
        }
    };
    let metrics = engine
        .complexity(&target.id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text_content(&json!({
        "function": target,
        "metrics": metrics,
    })))
}

// Suppress unused import warning when none of the error variants are
// referenced by name in this file (we only print them via Display).
#[allow(dead_code)]
fn _graph_error_marker(e: GraphError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Path to the built binary. Cargo sets `CARGO_BIN_EXE_<name>` for
    /// integration-test targets; for `mod tests` inside the bin we fall back
    /// to `<manifest>/target/debug/latte-mcp` (where Cargo drops the bin).
    fn binary() -> std::path::PathBuf {
        if let Some(p) = std::env::var_os("CARGO_BIN_EXE_latte-mcp") {
            return std::path::PathBuf::from(p);
        }
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .expect("CARGO_MANIFEST_DIR");
        let candidate = manifest.join("target").join("debug").join("latte-mcp");
        assert!(
            candidate.exists(),
            "latte-mcp binary not at {candidate:?}; build with `cargo build --bin latte-mcp` first"
        );
        candidate
    }

    /// Spawn the MCP server, write `input` to its stdin, return stdout as a String.
    fn run_server(input: &str) -> String {
        // Ensure the request is newline-terminated; the server reads lines.
        let mut payload = input.to_owned();
        if !payload.ends_with('\n') {
            payload.push('\n');
        }
        let mut child = Command::new(binary())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn latte-mcp");
        let mut stdin = child.stdin.take().expect("stdin handle");
        std::io::Write::write_all(&mut stdin, payload.as_bytes()).expect("write stdin");
        // Close stdin so the child sees EOF and exits the read loop.
        drop(stdin);
        let output = child.wait_with_output().expect("wait for output");
        if !output.status.success() {
            eprintln!(
                "latte-mcp stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn mcp_server_returns_initialize_with_server_info() {
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"x","version":"1"}}}"#;
        let stdout = run_server(input);
        let reply_line = stdout.lines().next().expect("a response line");
        let v: Value = serde_json::from_str(reply_line).expect("parse response");
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], "latte-mcp");
        assert!(v["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn mcp_server_lists_tools_with_required_schemas() {
        let input = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        let stdout = run_server(input);
        let reply_line = stdout.lines().next().expect("a response line");
        let v: Value = serde_json::from_str(reply_line).expect("parse response");
        let tools = v["result"]["tools"].as_array().expect("tools array");
        assert!(
            tools.len() >= 11,
            "expected at least 11 tools, got {}",
            tools.len()
        );
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        for required in &["search_graph", "query_graph", "complexity"] {
            assert!(
                names.contains(required),
                "tools/list must contain `{required}`, got {names:?}"
            );
        }
        // Every tool advertises an inputSchema of type "object".
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or("<noname>");
            assert_eq!(
                tool["inputSchema"]["type"], "object",
                "tool `{name}` missing inputSchema.type"
            );
        }
    }

    #[test]
    fn mcp_server_search_graph_against_real_db() {
        // Bootstrap a tiny Rust project on disk, build a graph against it,
        // then ask the MCP server to search it.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn alpha() -> i32 { 1 }\npub fn gamma() -> i32 { 2 }\n",
        )
        .unwrap();

        let db = root.join("graph.db");
        let storage = SqliteStorage::open(&db).expect("open db");
        let engine = TreeSitterEngine::new(storage);
        let opts = BuildOptions {
            project_root: root.to_string_lossy().to_string(),
            ..BuildOptions::default()
        };
        let report = futures_block_on(engine.build(root, &opts));
        assert!(
            report.is_ok(),
            "engine.build should succeed for a trivial project: {:?}",
            report.err()
        );

        let request = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "search_graph",
                "arguments": {
                    "db": db.to_string_lossy(),
                    "query": "alpha",
                    "limit": 10
                }
            }
        });
        let input = format!("{}\n", request);
        let stdout = run_server(&input);
        let reply_line = stdout.lines().next().expect("a response line");
        let v: Value = serde_json::from_str(reply_line).expect("parse response");
        assert_eq!(v["id"], 3);
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let body: Value = serde_json::from_str(text).expect("inner JSON");
        let hits = body["hits"].as_array().expect("hits array");
        assert!(
            !hits.is_empty(),
            "search_graph should return at least one hit, body = {body}"
        );
    }

    /// Minimal hand-rolled `block_on` so we don't pull in another dep just for tests.
    fn futures_block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
            .block_on(fut)
    }
}