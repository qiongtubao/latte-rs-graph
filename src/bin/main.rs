use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};

use latte_rs_graph::engine::TreeSitterEngine;
use latte_rs_graph::error::GraphResult;
use latte_rs_graph::storage::SqliteStorage;
use latte_rs_graph::traits::GraphProvider;
use latte_rs_graph::types::{
    ArchitectureAspect, ArchitectureRequest, BuildOptions, ComplexityMetrics,
    SearchCodeMode, SearchCodeRequest,
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
    /// Show function/method nodes with zero callers, excluding entry points.
    Dead {
        /// Path to the graph database
        db: PathBuf,
    },
    /// Map git changes to their blast radius (transitive callers).
    Blast {
        /// Path to the graph database
        db: PathBuf,
        /// Optional git ref (e.g. "HEAD~1"). If empty, falls back to `git status`.
        base: Option<String>,
        /// Max traversal depth from changed symbols (default 3)
        #[arg(short = 'l', long, default_value = "3")]
        depth: u32,
        /// Direction: inbound | outbound | both
        #[arg(short = 'D', long, default_value = "inbound")]
        direction: String,
    },
    /// Multi-aspect architecture overview of the indexed project (Phase 4).
    Arch {
        /// Path to the graph database
        db: PathBuf,
        /// Optional file_path prefix to scope the analysis (e.g. "src/foo")
        #[arg(long)]
        path: Option<String>,
        /// Comma-separated aspects to include (e.g. "overview,structure,hotspots").
        /// Empty = the default set (everything except clusters/cycles).
        #[arg(long)]
        aspects: Option<String>,
        /// Top-N cap for hotspots / boundaries / dependencies.
        #[arg(short = 'n', long, default_value = "20")]
        top_n: usize,
        /// Emit JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// Execute a Cypher query against the graph and print the result
    /// as JSON rows (Phase 5 subset).
    Cypher {
        /// Path to the graph database
        db: PathBuf,
        /// The Cypher query string. Wrap in quotes for multi-word queries.
        query: String,
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

        Command::Dead { db } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let report = engine.dead_code().await?;
            let live = report.total_functions.saturating_sub(report.dead_count)
                .saturating_sub(report.entry_points);
            println!(
                "💀 {} dead functions ({} total functions, {} entry points, {} live)",
                report.dead_count, report.total_functions, report.entry_points, live
            );
            if report.entries.is_empty() {
                println!("   ✓ no unreachable symbols");
            } else {
                for e in &report.entries {
                    println!(
                        "   {}:{}  {} {}",
                        e.node.file_path,
                        e.node.start_line,
                        e.node.kind.as_str(),
                        e.node.qualified_name,
                    );
                }
            }
        }

        Command::Blast { db, base, depth, direction } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);

            // Resolve changed paths via git. The trait/engine never shells
            // out — that's the CLI's job. Engine stays pure for testing.
            // Derive project root from the db location: db lives at
            // `<root>/.latte/graph.db`, so the project root is two levels up.
            let project_root = db
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let changed_paths = match resolve_git_changes(base.as_deref(), &project_root) {
                Ok(v) => v,
                Err(msg) => {
                    println!("⚠ {msg}");
                    return Ok(());
                }
            };
            if changed_paths.is_empty() {
                println!("⚠ no changed files detected (working tree clean or repo not initialised).");
                return Ok(());
            }

            let dir = parse_direction(&direction);
            let report = engine
                .blast_radius(changed_paths.clone(), depth, dir, base.clone())
                .await?;

            let dir_label = match dir {
                latte_rs_graph::types::BlastDirection::Inbound => "inbound",
                latte_rs_graph::types::BlastDirection::Outbound => "outbound",
                latte_rs_graph::types::BlastDirection::Both => "both",
            };
            println!("💥 Blast radius ({dir_label}, depth={depth})");
            println!("   Changed:        {} files", report.changed_files.len());
            let seed_count: usize = report.seeds.iter().map(|s| s.nodes.len()).sum();
            println!("   Seeds:          {} symbols", seed_count);
            println!(
                "   Impacted:       {} symbols across {} files",
                report.impact.total_count,
                report.impact.files.len()
            );
            if !report.impact.files.is_empty() {
                println!("   files:");
                for f in &report.impact.files {
                    println!("     - {f}");
                }
            }
            // Top symbols — first 20 deterministic (already sorted by engine).
            let head = report.impact.nodes.iter().take(20);
            println!("   top symbols:");
            for n in head {
                println!(
                    "     - {} {}  {}:{}",
                    n.kind.as_str(),
                    n.qualified_name,
                    n.file_path,
                    n.start_line,
                );
            }
        }

        Command::Arch { db, path, aspects, top_n, json } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let aspect_list = aspects
                .as_deref()
                .map(ArchitectureAspect::parse_list)
                .unwrap_or_default();
            let req = ArchitectureRequest {
                path_scope: path,
                aspects: aspect_list,
                top_n,
            };
            let report = engine.architecture_overview(&req).await?;
            if json {
                let s = serde_json::to_string_pretty(&report)
                    .map_err(latte_rs_graph::error::GraphError::Serde)?;
                println!("{s}");
            } else {
                render_arch_report(&report);
            }
        }
        Command::Cypher { db, query } => {
            let storage = SqliteStorage::open_readonly(&db)?;
            let engine = TreeSitterEngine::new(storage);
            let report = engine.cypher(&query).await?;
            let s = serde_json::to_string_pretty(&report)
                .map_err(latte_rs_graph::error::GraphError::Serde)?;
            println!("{s}");
        }
     }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI helpers — kept out of the trait so the engine stays pure / testable
// and the CLI shells out only at the user-interface boundary.
// ---------------------------------------------------------------------------

/// Run `git` from the current directory. If `--base` is given, returns
/// the output of `git diff --name-only <base>`; otherwise the union of
/// `git ls-files --modified --others --exclude-standard` (covers staged,
/// unstaged, and untracked). On non-zero exit / missing binary we return
/// a user-facing error string the caller can print and exit on.
fn resolve_git_changes(
    base: Option<&str>,
    project_root: &std::path::Path,
) -> Result<Vec<String>, String> {
    use std::process::Command;
    let git = |args: &[&str]| -> std::io::Result<std::process::Output> {
        Command::new("git").args(args).current_dir(project_root).output()
    };

    let raw = if let Some(b) = base {
        let out = git(&["diff", "--name-only", b])
            .map_err(|e| format!("failed to invoke git: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "`git diff --name-only {b}` exited with status {}: {stderr}",
                out.status
            ));
        }
        out.stdout
    } else {
        // Status porcelain: lines look like ` M src/foo.rs`, `?? src/bar.rs`.
        // Column 2 (1-indexed) holds the path.
        let out = git(&["status", "--porcelain"])
            .map_err(|e| format!("failed to invoke git: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "`git status --porcelain` exited with status {}: {stderr}",
                out.status
            ));
        }
        out.stdout
    };

    let text = String::from_utf8_lossy(&raw);
    let mut paths: Vec<String> = Vec::new();
    // Parse each output line into a path. Two formats:
    // same offset rule: drop the 3-char status prefix ("XY ") on
    // porcelain, drop nothing on plain name-only. Don't `trim()`
    // inside the porcelain branch — the column-2 separator IS
    // significant and gets eaten by trim, shifting our offset.
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let body = if base.is_some() {
            line.trim()
        } else if line.len() >= 3 {
            &line[3..]
        } else {
            ""
        };
        let final_path = if let Some(idx) = body.find(" -> ") {
            body[idx + 4..].trim()
        } else {
            body.trim()
        };
        if !final_path.is_empty() {
            paths.push(final_path.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Map the user-facing direction string to a typed enum.
fn parse_direction(s: &str) -> latte_rs_graph::types::BlastDirection {
    match s.to_ascii_lowercase().as_str() {
        "outbound" => latte_rs_graph::types::BlastDirection::Outbound,
        "both" => latte_rs_graph::types::BlastDirection::Both,
        _ => latte_rs_graph::types::BlastDirection::Inbound,
    }
}

// ---------------------------------------------------------------------------
// Architecture report rendering (Phase 4)
// ---------------------------------------------------------------------------
//
/// Pretty-print an architecture report to stdout. Sections appear in a fixed
/// order; sections that weren't requested are skipped. Used when the user
/// runs `lrg arch` without `--json`.
fn render_arch_report(report: &latte_rs_graph::types::ArchitectureReport) {
    use latte_rs_graph::types::ArchitectureAspect;

    let wanted: std::collections::HashSet<ArchitectureAspect> =
        report.requested_aspects.iter().copied().collect();

    if wanted.contains(&ArchitectureAspect::Overview) {
        if let Some(o) = &report.overview {
            println!("📊 overview");
            println!("   total_nodes        = {}", o.total_nodes);
            println!("   total_edges        = {}", o.total_edges);
            println!("   total_files        = {}", o.total_files);
            println!("   total_routes       = {}", o.total_routes);
            println!("   languages_count    = {}", o.languages_count);
            println!("   entry_points_count = {}", o.entry_points_count);
            println!("   dead_count         = {}", o.dead_count);
            println!("   call_edges_count   = {}", o.call_edges_count);
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Structure) {
        if let Some(rows) = &report.structure {
            println!("📦 structure");
            for r in rows {
                println!("   {p}  {n} nodes  {e} edges",
                    p = r.path,
                    n = r.node_count,
                    e = r.edge_count);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Packages) {
        if let Some(rows) = &report.packages {
            println!("📦 packages");
            for r in rows {
                println!("   {p}  {n} nodes  {e} edges",
                    p = r.path,
                    n = r.node_count,
                    e = r.edge_count);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Dependencies) {
        if let Some(rows) = &report.dependencies {
            println!("🔗 dependencies");
            for r in rows {
                println!("   {from} -> {to}  {n} edges",
                    from = r.from_path,
                    to = r.to_path,
                    n = r.edge_count);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Routes) {
        if let Some(rows) = &report.routes {
            println!("🛣  routes");
            for r in rows {
                let method_path = match (&r.method, &r.path) {
                    (Some(m), Some(p)) => format!("{m} {p}"),
                    (Some(m), None) => m.clone(),
                    (None, Some(p)) => p.clone(),
                    _ => String::new(),
                };
                if method_path.is_empty() {
                    println!("   {f}:{l}  {qn}",
                        f = r.file_path,
                        l = r.start_line,
                        qn = r.qualified_name);
                } else {
                    println!("   {f}:{l}  {qn}  {mp}",
                        f = r.file_path,
                        l = r.start_line,
                        qn = r.qualified_name,
                        mp = method_path);
                }
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Languages) {
        if let Some(rows) = &report.languages {
            println!("🌐 languages");
            for r in rows {
                println!("   {lang}  nodes={n}  edges={e}  files={f}",
                    lang = r.language,
                    n = r.node_count,
                    e = r.edge_count,
                    f = r.file_count);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::EntryPoints) {
        if let Some(rows) = &report.entry_points {
            println!("🚪 entry_points");
            for n in rows {
                println!("   {f}:{l}  {k} {qn}",
                    f = n.file_path,
                    l = n.start_line,
                    k = n.kind.as_str(),
                    qn = n.qualified_name);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Hotspots) {
        if let Some(rows) = &report.hotspots {
            println!("🔥 hotspots");
            for r in rows {
                println!("   {qn}  in_degree={d}  {f}:{l}",
                    qn = r.node.qualified_name,
                    d = r.in_degree,
                    f = r.node.file_path,
                    l = r.node.start_line);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Boundaries) {
        if let Some(rows) = &report.boundaries {
            println!("🚧 boundaries");
            for r in rows {
                println!("   {p}  in={i}  out={o}  fan_ratio={ratio:.2}",
                    p = r.path,
                    i = r.inbound_count,
                    o = r.outbound_count,
                    ratio = r.fan_out_ratio);
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Layers) {
        if let Some(l) = &report.layers {
            println!("🪜 layers");
            println!("   entries={e}  max_layer={m}",
                e = l.entry_count,
                m = l.max_layer);
            for (i, c) in l.node_count_per_layer.iter().enumerate() {
                println!("   layer {i}: {c}");
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::FileTree) {
        if let Some(t) = &report.file_tree {
            println!("🌳 file_tree (root = {})", t.root);
            for entry in &t.entries {
                println!("   {}", entry.name);
                let last = entry.children.len();
                for (i, child) in entry.children.iter().enumerate() {
                    let branch = if i + 1 == last { "└──" } else { "├──" };
                    println!("     {branch} {}", child.name);
                }
            }
            println!();
        }
    }

    if wanted.contains(&ArchitectureAspect::Clusters) || wanted.contains(&ArchitectureAspect::Cycles) {
        println!("⚠ clusters/cycles not yet implemented (Phase 5)");
    }
}
