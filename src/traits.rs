use async_trait::async_trait;
use std::path::Path;

use crate::error::GraphResult;
use crate::types::{
    ArchitectureReport, ArchitectureRequest, BlastDirection, BlastRadiusReport,
    BuildOptions, BuildReport, ComplexityMetrics, DeadCodeReport, GraphData, GraphStats, Node,
    RelationResult, SearchCodeRequest, SearchCodeResponse, SearchOptions, UpdateReport,
};

/// Core trait that every graph engine must implement.
///
/// Engines are swappable at runtime — the editor can choose between
/// the native tree-sitter engine, a sidecar bridge, or a remote backend.
///
/// All methods are async so engines that delegate to external processes
/// (e.g. calling a Python sidecar) do not block the runtime.
#[async_trait]
pub trait GraphProvider: Send + Sync {
    /// Human-readable engine name (e.g. "tree-sitter", "codegraph-bridge").
    fn name(&self) -> &str;

    /// List of programming languages this engine supports.
    fn supported_languages(&self) -> Vec<&str>;

    // =====================================================================
    // Build & Update
    // =====================================================================

    /// Build the full code graph for a project at `root`.
    async fn build(&self, root: &Path, options: &BuildOptions) -> GraphResult<BuildReport>;

    /// Incrementally update the graph (e.g. after file changes).
    async fn update(&self, root: &Path) -> GraphResult<UpdateReport>;

    // =====================================================================
    // Query
    // =====================================================================

    /// Return the full graph data (all nodes + edges).
    async fn graph_data(&self) -> GraphResult<GraphData>;

    /// Search nodes by name / qualified_name (FTS + fuzzy).
    async fn search(&self, query: &str, options: &SearchOptions) -> GraphResult<Vec<Node>>;

    /// Find definition nodes by exact name match.
    async fn find_definitions(&self, name: &str) -> GraphResult<Vec<Node>>;

    /// Get a node by its ID.
    async fn node_by_id(&self, id: &str) -> GraphResult<Option<Node>>;

    /// Get subgraph around a node (neighbors up to `depth`).
    async fn subgraph(&self, node_id: &str, depth: u32) -> GraphResult<GraphData>;

    /// Get graph statistics.
    async fn stats(&self) -> GraphResult<GraphStats>;
    /// Graph-augmented code search. Reads each indexed source file, finds
    /// literal whitespace-AND substring matches, and attaches the smallest
    /// enclosing definition node. See `query::grep::search_code`.
    async fn search_code(
        &self,
        project_root: &Path,
        req: &SearchCodeRequest,
    ) -> GraphResult<SearchCodeResponse>;

    /// Look up the complexity metrics previously stored on a function node
    /// (cyclomatic, cognitive, max loop depth, alloc/linear-scan-in-loop,
    async fn complexity(&self, node_id: &str) -> GraphResult<Option<ComplexityMetrics>>;

    /// Return all `function`/`method`/`test` nodes with no `calls` inbound
    /// edge and no entry-point status. Sorted by `file_path` then `name`.
    async fn dead_code(&self) -> GraphResult<DeadCodeReport>;

    /// Given a list of changed file paths (relative to project root),
    /// return the symbols defined in them and their transitive `calls`
    /// reachability set. `direction` defaults to `Inbound` (callers).
    /// `base_ref` is just propagated into the report — the engine does not
    /// resolve git refs itself.
    async fn blast_radius(
        &self,
        changed_paths: Vec<String>,
        depth: u32,
        direction: BlastDirection,
        base_ref: Option<String>,
    ) -> GraphResult<BlastRadiusReport>;


    // =====================================================================
    // Analysis
    // =====================================================================

    /// Get callers of a node (reverse edges).
    async fn callers(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult>;

    /// Get callees of a node (outgoing edegs).
    async fn callees(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult>;

    /// Clear all cached data for this engine.
    async fn clear(&self) -> GraphResult<()>;

    /// Multi-aspect architecture query (Phase 4). Returns a structured
    /// report covering whatever aspects were requested in `req` (empty
    /// aspect list = the default set, which excludes the deferred
    /// `Clusters`/`Cycles`). Path scoping is honoured by the storage layer.
    async fn architecture_overview(
        &self,
        req: &ArchitectureRequest,
    ) -> GraphResult<ArchitectureReport>;
}

/// Extension trait for engines that support community detection.
#[async_trait]
pub trait CommunityDetection: GraphProvider {
    async fn communities(&self) -> GraphResult<Vec<Community>>;
}

#[derive(Debug, Clone)]
pub struct Community {
    pub id: usize,
    pub name: String,
    pub node_ids: Vec<String>,
}
