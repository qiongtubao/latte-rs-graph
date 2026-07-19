use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

use latte_rs_graph::engine::TreeSitterEngine;
use latte_rs_graph::error::GraphResult;
use latte_rs_graph::storage::SqliteStorage;
use latte_rs_graph::traits::GraphProvider;
use latte_rs_graph::types::{
    BuildOptions, ComplexityMetrics, SearchCodeMode, SearchCodeRequest,
};

#[derive(Parser)]
#[command(name = "lrg", about = "latte-rs-graph CLI — code graph builder & query tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a code graph for a project
    Build {
        /// Path to project root
        path: PathBuf,
        /// Path for the graph database (default: <project>/.latte/graph.db)
        #[arg(short, long)]
        db: Option<PathBuf>,
        /// Languages to parse (comma-separated, default: all)
        #[arg(short, long)]
        langs: Option<String>,
    },
    /// Show graph statistics
    Stats {
        /// Path to the graph database
        db: PathBuf,
    },
    /// Search the graph
    Search {
        /// Path to the graph database
        db: PathBuf,
        /// Search query
        query: String,
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Export graph data as JSON
    Export {
        /// Path to the graph database
        db: PathBuf,
        /// Optional limit on nodes
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Show subgraph around a node
    Subgraph {
        /// Path to the graph database
        db: PathBuf,
        /// Node ID to center on
        node: String,
        /// Traversal depth
        #[arg(short, long, default_value = "2")]
        depth: u32,
    },
    /// Graph-augmented code search: literal substring matches with the enclosing
    /// graph node attached. Sorts definitions > popular > tests.
    Grep {
        /// Path to the graph database
        db: PathBuf,
        /// Project root (the source tree that was indexed)
        root: PathBuf,
        /// Pattern (whitespace AND match, literal substring)
        pattern: String,
        /// Reserved for regex (kept literal in v1)
        #[arg(long, default_value = "false")]
        regex: bool,
        /// Comma-separated file extensions to include (e.g. "rs,ts")
        #[arg(long)]
        ext: Option<String>,
        /// Anchor regex on file path (e.g. "^src/")
        #[arg(long)]
        path_filter: Option<String>,
        /// Context lines around each hit
        #[arg(long, default_value = "3")]
        context: u32,
        /// Max number of deduplicated results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show the complexity metrics computed for a single function/method
    Complexity {
        /// Path to the graph database
        db: PathBuf,
        /// Symbol name (function/class) to look up
        name: String,
    },
 }

#[tokio::main]
async fn main() -> GraphResult<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Build { path, db, langs } => {
            let project_root = path.canonicalize().map_err(|e| {
                latte_rs_graph::error::GraphError::Io(e)
            })?;

            let db_path = db.unwrap_or_else(|| {
                let mut p = project_root.clone();
                p.push(".latte");
                std::fs::create_dir_all(&p).ok();
                p.push("graph.db");
                p
            });

            println!("📦 Project:      {}", project_root.display());
            println!("📁 Graph DB:     {}", db_path.display());
            println!();

            let storage = SqliteStorage::open(&db_path)?;
            let engine = TreeSitterEngine::new(storage);

            let mut options = BuildOptions::default();
            options.project_root = project_root.to_string_lossy().to_string();
            if let Some(l) = langs {
                options.languages = Some(l.split(',').map(|s| s.trim().to_string()).collect());
            }

            let start = Instant::now();
            let report = engine.build(&project_root, &options).await?;
            let elapsed = start.elapsed();

            println!("✅ Build complete in {:.2}s", elapsed.as_secs_f64());
            println!("   Files scanned:  {}", report.files_scanned);
            println!("   Nodes created:  {}", report.nodes_created);
            println!("   Edges created:  {}", report.edges_created);
            if !report.errors.is_empty() {
                println!("   Errors ({}):", report.errors.len());
                for e in &report.errors[..report.errors.len().min(10)] {
                    println!("     ⚠ {}", e);
                }
            }

            // Print stats
            let stats = engine.stats().await?;
            println!();
            println!("📊 Graph statistics:");
            println!("   Total nodes: {}", stats.total_nodes);
            println!("   Total edges: {}", stats.total_edges);
            println!("   Total files: {}", stats.total_files);
            if !stats.node_kinds.is_empty() {
                println!("   Node kinds:");
                for k in &stats.node_kinds {
                    println!("     {:>8}  {}", k.count, k.kind);
                }
            }
        }

        Command::Stats { db } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let stats = engine.stats().await?;

            println!("📊 Graph statistics for: {}", db.display());
            println!("   Total nodes: {}", stats.total_nodes);
            println!("   Total edges: {}", stats.total_edges);
            println!("   Total files: {}", stats.total_files);
            if !stats.node_kinds.is_empty() {
                println!();
                println!("   By kind:");
                for k in &stats.node_kinds {
                    println!("     {:>8}  {}", k.count, k.kind);
                }
            }
            if !stats.edge_kinds.is_empty() {
                println!();
                println!("   By edge kind:");
                for k in &stats.edge_kinds {
                    println!("     {:>8}  {}", k.count, k.kind);
                }
            }
        }

        Command::Search { db, query, limit } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let results = engine
                .search(&query, &latte_rs_graph::types::SearchOptions { limit, ..Default::default() })
                .await?;

            println!("🔍 Search results for \"{}\":", query);
            if results.is_empty() {
                println!("   No results found.");
            } else {
                for (i, node) in results.iter().enumerate() {
                    println!(
                        "   {}. {} {} ({})",
                        i + 1,
                        node.kind.as_str(),
                        node.name,
                        node.file_path
                    );
                }
                println!("   ({} total)", results.len());
            }
        }

        Command::Export { db, limit } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let mut data = engine.graph_data().await?;

            if let Some(lim) = limit {
                data.nodes.truncate(lim);
                data.edges.truncate(lim * 2);
            }

            let json = serde_json::to_string_pretty(&data)
                .map_err(|e| latte_rs_graph::error::GraphError::Serde(e))?;
            println!("{}", json);
        }

        Command::Subgraph { db, node, depth } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let data = engine.subgraph(&node, depth).await?;

            println!("🔗 Subgraph around \"{}\" (depth={}):", node, depth);
            println!("   Nodes: {}", data.nodes.len());
            println!("   Edges: {}", data.edges.len());
            for n in &data.nodes {
                let sig = n
                    .signature
                    .as_deref()
                    .unwrap_or(&n.name);
                println!("   • {} {}", n.kind.as_str(), sig);
            }
        }

        Command::Grep { db, root, pattern, regex, ext, path_filter, context, limit } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let file_extensions = ext.map(|s| {
                s.split(',').map(|t| t.trim().to_string()).collect::<Vec<_>>()
            });
            let req = SearchCodeRequest {
                pattern,
                regex,
                file_extensions,
                path_filter,
                mode: SearchCodeMode::Compact,
                context_lines: context,
                limit,
            };
            let resp = engine.search_code(&root, &req).await?;

            println!(
                "🔍 {} raw hits → {} deduplicated (limit {})",
                resp.total_grep_matches, resp.total_results, limit
            );
            if resp.truncated {
                println!("   ⚠ truncated; raise --limit or narrow with --ext/--path-filter");
            }
            for m in &resp.results {
                let kind_label = match m.hit_kind {
                    latte_rs_graph::types::HitKind::Definition => "DEF",
                    latte_rs_graph::types::HitKind::Usage => "USE",
                    latte_rs_graph::types::HitKind::Test => "TST",
                };
                let enclosing = m
                    .containing_node
                    .as_ref()
                    .map(|n| format!("{} {}", n.kind.as_str(), n.qualified_name))
                    .unwrap_or_else(|| "<top-level>".to_string());
                println!(
                    "   [{:>3}] {} {} : {} (×{} match{}, in-degree {})",
                    kind_label,
                    m.file_path,
                    m.line,
                    enclosing,
                    m.match_count,
                    if m.match_count == 1 { "" } else { "es" },
                    m.in_degree,
                );
                if !m.snippet.is_empty() {
                    for ln in m.snippet.lines() {
                        println!("         | {ln}");
                    }
                }
            }
        }

        Command::Complexity { db, name } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let matches = engine.find_definitions(&name).await?;
            if matches.is_empty() {
                println!("⚠ No definitions found for \"{name}\".");
                return Ok(());
            }
            for n in &matches {
                let m: Option<ComplexityMetrics> = engine.complexity(&n.id).await?;
                println!("📐 {} {}", n.kind.as_str(), n.qualified_name);
                println!("   {} : {}", n.file_path, n.start_line);
                if let Some(c) = m {
                    println!("   cyclomatic            : {}", c.cyclomatic);
                    println!("   cognitive             : {}", c.cognitive);
                    println!("   max_loop_depth        : {}", c.max_loop_depth);
                    println!("   alloc_in_loop         : {}", c.alloc_in_loop);
                    println!("   linear_scan_in_loop   : {}", c.linear_scan_in_loop);
                    println!("   is_recursive          : {}", c.is_recursive);
                    println!("   unguarded_recursion   : {}", c.unguarded_recursion);
                } else {
                    println!("   (no metrics — node may not be a function)");
                }
                println!();
            }
        }
     }

    Ok(())
}
