use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::{GraphError, GraphResult};
use crate::types::*;

/// In-memory graph storage for testing and small graphs.
pub struct MemoryStorage {
    nodes: RwLock<HashMap<String, Node>>,
    edges: RwLock<Vec<Edge>>,
    files: RwLock<HashMap<String, FileRecord>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            edges: RwLock::new(Vec::new()),
            files: RwLock::new(HashMap::new()),
        }
    }

    pub fn upsert_node(&self, node: Node) -> GraphResult<()> {
        self.nodes.write().map_err(|e| GraphError::Engine(e.to_string()))?.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn upsert_nodes_batch(&self, nodes: &[Node]) -> GraphResult<()> {
        let mut map = self.nodes.write().map_err(|e| GraphError::Engine(e.to_string()))?;
        for node in nodes {
            map.insert(node.id.clone(), node.clone());
        }
        Ok(())
    }

    pub fn upsert_edge(&self, edge: Edge) -> GraphResult<()> {
        self.edges.write().map_err(|e| GraphError::Engine(e.to_string()))?.push(edge);
        Ok(())
    }

    pub fn upsert_edges_batch(&self, edges: &[Edge]) -> GraphResult<()> {
        self.edges.write().map_err(|e| GraphError::Engine(e.to_string()))?.extend_from_slice(edges);
        Ok(())
    }

    pub fn all_nodes(&self) -> GraphResult<Vec<Node>> {
        Ok(self.nodes.read().map_err(|e| GraphError::Engine(e.to_string()))?.values().cloned().collect())
    }

    pub fn all_edges(&self) -> GraphResult<Vec<Edge>> {
        Ok(self.edges.read().map_err(|e| GraphError::Engine(e.to_string()))?.clone())
    }

    pub fn search_nodes(&self, query: &str, limit: usize) -> GraphResult<Vec<Node>> {
        let q = query.to_lowercase();
        let nodes = self.nodes.read().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut results: Vec<Node> = nodes
            .values()
            .filter(|n| {
                n.name.to_lowercase().contains(&q)
                    || n.qualified_name.to_lowercase().contains(&q)
                    || n.file_path.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        results.sort_by(|a, b| a.name.cmp(&b.name));
        results.truncate(limit);
        Ok(results)
    }

    pub fn find_definitions(&self, name: &str) -> GraphResult<Vec<Node>> {
        let nodes = self.nodes.read().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut results: Vec<Node> = nodes
            .values()
            .filter(|n| {
                n.kind != NodeKind::File
                    && n.kind != NodeKind::Import
                    && n.kind != NodeKind::Export
                    && (n.name == name || n.qualified_name == name)
            })
            .cloned()
            .collect();
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    pub fn node_by_id(&self, id: &str) -> GraphResult<Option<Node>> {
        Ok(self.nodes.read().map_err(|e| GraphError::Engine(e.to_string()))?.get(id).cloned())
    }

    pub fn edges_for_node(&self, node_id: &str) -> GraphResult<Vec<Edge>> {
        Ok(self.edges.read().map_err(|e| GraphError::Engine(e.to_string()))?
            .iter()
            .filter(|e| e.source == node_id || e.target == node_id)
            .cloned()
            .collect())
    }

    pub fn stats(&self) -> GraphResult<GraphStats> {
        let nodes = self.nodes.read().map_err(|e| GraphError::Engine(e.to_string()))?;
        let edges = self.edges.read().map_err(|e| GraphError::Engine(e.to_string()))?;
        let files = self.files.read().map_err(|e| GraphError::Engine(e.to_string()))?;

        let mut kind_map: HashMap<String, usize> = HashMap::new();
        for n in nodes.values() {
            *kind_map.entry(n.kind.as_str().to_string()).or_default() += 1;
        }
        let mut node_kinds: Vec<KindCount> = kind_map
            .into_iter()
            .map(|(kind, count)| KindCount { kind, count })
            .collect();
        node_kinds.sort_by(|a, b| b.count.cmp(&a.count));

        let mut ek_map: HashMap<String, usize> = HashMap::new();
        for e in edges.iter() {
            *ek_map.entry(e.kind.as_str().to_string()).or_default() += 1;
        }
        let mut edge_kinds: Vec<KindCount> = ek_map
            .into_iter()
            .map(|(kind, count)| KindCount { kind, count })
            .collect();
        edge_kinds.sort_by(|a, b| b.count.cmp(&a.count));

        Ok(GraphStats {
            total_nodes: nodes.len(),
            total_edges: edges.len(),
            total_files: files.len(),
            node_kinds,
            edge_kinds,
        })
    }

    pub fn clear_all(&self) -> GraphResult<()> {
        self.nodes.write().map_err(|e| GraphError::Engine(e.to_string()))?.clear();
        self.edges.write().map_err(|e| GraphError::Engine(e.to_string()))?.clear();
        self.files.write().map_err(|e| GraphError::Engine(e.to_string()))?.clear();
        Ok(())
    }

    /// Smallest enclosing definition node covering `line` in `file_path`.
    pub fn containing_node_for_line(
        &self,
        file_path: &str,
        line: u32,
    ) -> GraphResult<Option<Node>> {
        let nodes = self
            .nodes
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        Ok(nodes
            .values()
            .filter(|node| {
                node.file_path == file_path
                    && node.start_line <= line
                    && node.end_line >= line
                    && matches!(
                        &node.kind,
                        NodeKind::Function
                            | NodeKind::Method
                            | NodeKind::Class
                            | NodeKind::Trait
                            | NodeKind::Interface
                            | NodeKind::Struct
                            | NodeKind::Route
                    )
            })
            .min_by_key(|node| node.end_line.saturating_sub(node.start_line))
            .cloned())
    }

    /// Inbound CALLS edge count for ranking.
    pub fn in_degree_calls(&self, node_id: &str) -> GraphResult<u32> {
        let edges = self
            .edges
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        Ok(edges
            .iter()
            .filter(|edge| edge.target == node_id && edge.kind == EdgeKind::Calls)
            .count() as u32)
    }
}
