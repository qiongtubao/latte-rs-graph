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
    // =========================================================================
    // Dead-code + Blast-radius analysis (Phase 2) — in-memory mirrors
    // =========================================================================

    /// Mirrors `SqliteStorage::dead_code`. Empty `reasons_excluded_from_entry`
    /// because the SQL filter naturally excludes entry points.
    pub fn dead_code(&self) -> GraphResult<Vec<DeadCodeEntry>> {
        let nodes = self
            .nodes
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        let edges = self
            .edges
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;

        // Nodes that receive any `calls` edge.
        let called: std::collections::HashSet<&str> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| e.target.as_str())
            .collect();

        let mut entries: Vec<DeadCodeEntry> = nodes
            .values()
            .filter(|n| {
                matches!(n.kind, NodeKind::Function | NodeKind::Method | NodeKind::Test)
                    && !called.contains(n.id.as_str())
                    && !is_entry_point(n)
            })
            .cloned()
            .map(|n| DeadCodeEntry { node: n, reasons_excluded_from_entry: Vec::new() })
            .collect();
        entries.sort_by(|a, b| {
            a.node
                .file_path
                .cmp(&b.node.file_path)
                .then_with(|| a.node.name.cmp(&b.node.name))
        });
        Ok(entries)
    }

    /// Mirrors `SqliteStorage::nodes_for_files`. Definition kinds only;
    /// files with zero matches are skipped; output order follows `paths`.
    pub fn nodes_for_files(&self, paths: &[String]) -> GraphResult<Vec<BlastSeed>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let nodes = self
            .nodes
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut by_file: std::collections::BTreeMap<&str, Vec<Node>> =
            std::collections::BTreeMap::new();
        for n in nodes.values() {
            if !is_definition_kind(&n.kind) {
                continue;
            }
            if let Some(list) = by_file.get_mut(n.file_path.as_str()) {
                list.push(n.clone());
            } else if paths.iter().any(|p| p == &n.file_path) {
                by_file.insert(n.file_path.as_str(), vec![n.clone()]);
            }
        }
        let mut seeds: Vec<BlastSeed> = Vec::new();
        for path in paths {
            if let Some(list) = by_file.remove(path.as_str()) {
                seeds.push(BlastSeed { file_path: path.clone(), nodes: list });
            }
        }
        Ok(seeds)
    }

    /// Mirrors `SqliteStorage::reachable_calls`. BFS over `Calls` edges
    /// starting from `seeds`, capped at `depth` hops, seeds excluded.
    pub fn reachable_calls(
        &self,
        seeds: &[String],
        depth: u32,
        direction: BlastDirection,
    ) -> GraphResult<BlastImpact> {
        if seeds.is_empty() || depth == 0 {
            return Ok(BlastImpact {
                nodes: Vec::new(),
                files: Vec::new(),
                total_count: 0,
            });
        }
        let edges = self
            .edges
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        let nodes = self
            .nodes
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;

        let seed_set: std::collections::HashSet<&str> = seeds.iter().map(|s| s.as_str()).collect();
        let mut impacted_ids: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let mut frontier: Vec<String> = seeds.to_vec();
        let mut visited: std::collections::HashSet<String> = seed_set.iter().map(|s| s.to_string()).collect();

        for _hop in 0..depth {
            let mut next: Vec<String> = Vec::new();
            for current in &frontier {
                for e in edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
                    let hop_to = match direction {
                        BlastDirection::Inbound if e.target == *current => Some(e.source.clone()),
                        BlastDirection::Outbound if e.source == *current => Some(e.target.clone()),
                        BlastDirection::Both => {
                            if e.target == *current {
                                Some(e.source.clone())
                            } else if e.source == *current {
                                Some(e.target.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(id) = hop_to {
                        if visited.insert(id.clone()) {
                            impacted_ids.insert(id.clone());
                            next.push(id);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        let mut impacted_nodes: Vec<Node> = Vec::with_capacity(impacted_ids.len());
        let mut files: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for id in &impacted_ids {
            if let Some(n) = nodes.get(id) {
                impacted_nodes.push(n.clone());
                files.insert(n.file_path.clone());
            }
        }
        impacted_nodes.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.qualified_name.cmp(&b.qualified_name))
        });
        let total_count = impacted_nodes.len();
        let files: Vec<String> = files.into_iter().collect();
        Ok(BlastImpact { nodes: impacted_nodes, files, total_count })
    }

    /// Count of entry-point nodes (mirrors `SqliteStorage::entry_point_count`).
    pub fn entry_point_count(&self) -> GraphResult<usize> {
        let nodes = self
            .nodes
            .read()
            .map_err(|e| GraphError::Engine(e.to_string()))?;
        Ok(nodes.values().filter(|n| is_entry_point(n)).count())
    }
}

// ---------------------------------------------------------------------------
// Free helpers local to memory storage (mirror SQLite row heuristics).
// ---------------------------------------------------------------------------

fn is_entry_point(n: &Node) -> bool {
    if matches!(n.name.as_str(), "main" | "index" | "__init__") {
        return true;
    }
    if n.is_exported {
        return true;
    }
    if matches!(n.visibility.as_deref(), Some("public") | Some("pub")) {
        return true;
    }
    if n.file_path.contains("/bin/") || n.file_path.contains("main.") || n.file_path.contains("index.") {
        return true;
    }
    false
}

fn is_definition_kind(k: &NodeKind) -> bool {
    matches!(
        k,
        NodeKind::Function
            | NodeKind::Method
            | NodeKind::Class
            | NodeKind::Struct
            | NodeKind::Trait
            | NodeKind::Interface
            | NodeKind::Route
            | NodeKind::Constructor
    )
}
