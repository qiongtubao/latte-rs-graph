use async_trait::async_trait;
use std::path::Path;

use crate::error::GraphResult;
use crate::types::{
    BuildOptions, BuildReport, GraphData, GraphStats, Node, RelationResult, SearchOptions,
    UpdateReport,
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

    // =====================================================================
    // Analysis
    // =====================================================================

    /// Get callers of a node (reverse edges).
    async fn callers(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult>;

    /// Get callees of a node (outgoing edegs).
    async fn callees(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult>;

    /// Clear all cached data for this engine.
    async fn clear(&self) -> GraphResult<()>;
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
