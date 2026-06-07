use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

use latte_rs_graph::engine::TreeSitterEngine;
use latte_rs_graph::error::GraphResult;
use latte_rs_graph::storage::SqliteStorage;
use latte_rs_graph::traits::GraphProvider;
use latte_rs_graph::types::BuildOptions;

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
    }

    Ok(())
}
