use crate::error::GraphResult;
use crate::query::traversal::bfs;
use crate::types::*;

/// Extract a subgraph centered on a node, up to `depth` hops.
///
/// Delegates to BFS traversal under the hood.
pub fn extract_subgraph(
    graph_data: &GraphData,
    center_id: &str,
    depth: u32,
) -> GraphResult<GraphData> {
    bfs(
        &graph_data.nodes,
        &graph_data.edges,
        &[center_id.to_string()],
        depth,
    )
}
