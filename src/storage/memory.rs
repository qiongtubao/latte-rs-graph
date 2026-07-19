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

    // =========================================================================
    // Architecture overview (Phase 4) — in-memory mirror
    // =========================================================================
    //
    // Best-effort port of the SQLite aggregation: the in-memory maps are
    // small enough that we can just bucket in Rust instead of SQL. The
    // output shape is identical to the SQLite one so callers (CLI, JSON
    // exporters) see the same report regardless of backend.

    pub fn architecture_overview(
        &self,
        req: &ArchitectureRequest,
    ) -> GraphResult<ArchitectureReport> {
        let aspects = if req.aspects.is_empty() {
            ArchitectureAspect::default_set()
        } else {
            req.aspects.clone()
        };
        let top_n = req.top_n.max(1);
        let wants = |a: ArchitectureAspect| aspects.contains(&a);

        let all_nodes = self.all_nodes()?;
        let all_edges = self.all_edges()?;
        let scoped_nodes: Vec<Node> = match &req.path_scope {
            Some(scope) => all_nodes
                .into_iter()
                .filter(|n| n.file_path == *scope || n.file_path.starts_with(&format!("{scope}/")))
                .collect(),
            None => all_nodes,
        };
        let id_set: std::collections::HashSet<&str> =
            scoped_nodes.iter().map(|n| n.id.as_str()).collect();
        let scoped_edges: Vec<&Edge> = match &req.path_scope {
            Some(scope) => all_edges
                .iter()
                .filter(|e| {
                    let s = id_to_file(e.source.as_str(), &scoped_nodes);
                    let t = id_to_file(e.target.as_str(), &scoped_nodes);
                    s.map(|p| p == *scope || p.starts_with(&format!("{scope}/"))).unwrap_or(false)
                        || t.map(|p| p == *scope || p.starts_with(&format!("{scope}/"))).unwrap_or(false)
                })
                .collect(),
            None => all_edges.iter().collect(),
        };

        let mut report = ArchitectureReport {
            requested_aspects: aspects.clone(),
            path_scope: req.path_scope.clone(),
            ..Default::default()
        };

        // ----- Overview ----------------------------------------------------
        if wants(ArchitectureAspect::Overview) {
            let total_edges = scoped_edges.len();
            let total_files = scoped_nodes
                .iter()
                .map(|n| n.file_path.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let total_routes = scoped_nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Route)
                .count();
            let languages_count = scoped_nodes
                .iter()
                .map(|n| n.language.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let entry_points_count = scoped_nodes.iter().filter(|n| is_entry_point(n)).count();
            // Dead count: callable kinds with zero inbound `calls` AND not
            // an entry point — mirrors the SQLite subquery.
            let called: std::collections::HashSet<&str> = scoped_edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .map(|e| e.target.as_str())
                .collect();
            let dead_count = scoped_nodes
                .iter()
                .filter(|n| {
                    matches!(n.kind, NodeKind::Function | NodeKind::Method | NodeKind::Test)
                        && !called.contains(n.id.as_str())
                        && !is_entry_point(n)
                })
                .count();
            let call_edges_count = scoped_edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .count();
            report.overview = Some(OverviewSection {
                total_nodes: scoped_nodes.len(),
                total_edges,
                total_files,
                total_routes,
                languages_count,
                entry_points_count,
                dead_count,
                call_edges_count,
            });
        }

        // Indexes that the per-aspect sections need.
        let id_to_node: std::collections::HashMap<&str, &Node> =
            scoped_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let _ = id_set;

        // ----- Structure / Packages ---------------------------------------
        if wants(ArchitectureAspect::Structure) || wants(ArchitectureAspect::Packages) {
            let mut pkg_nodes: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for n in &scoped_nodes {
                *pkg_nodes.entry(package_of(&n.file_path, 2)).or_default() += 1;
            }
            let mut pkg_edges: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for e in &scoped_edges {
                if let (Some(s), Some(t)) = (id_to_node.get(e.source.as_str()), id_to_node.get(e.target.as_str())) {
                    if s.file_path == t.file_path {
                        continue;
                    }
                    let sp = package_of(&s.file_path, 2);
                    let tp = package_of(&t.file_path, 2);
                    if sp == tp {
                        *pkg_edges.entry(sp).or_default() += 1;
                    }
                }
            }
            let mut rows: Vec<PackageRow> = pkg_nodes
                .into_iter()
                .map(|(path, node_count)| PackageRow {
                    edge_count: *pkg_edges.get(&path).unwrap_or(&0),
                    node_count,
                    path,
                })
                .collect();
            rows.sort_by(|a, b| b.node_count.cmp(&a.node_count).then(a.path.cmp(&b.path)));
            if wants(ArchitectureAspect::Structure) {
                report.structure = Some(rows.clone());
            }
            if wants(ArchitectureAspect::Packages) {
                rows.truncate(top_n);
                report.packages = Some(rows);
            }
        }

        // ----- Dependencies -----------------------------------------------
        if wants(ArchitectureAspect::Dependencies) {
            let mut dep: std::collections::BTreeMap<(String, String), usize> =
                std::collections::BTreeMap::new();
            for e in scoped_edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
                if let (Some(s), Some(t)) = (id_to_node.get(e.source.as_str()), id_to_node.get(e.target.as_str())) {
                    if s.file_path == t.file_path {
                        continue;
                    }
                    *dep.entry((s.file_path.clone(), t.file_path.clone())).or_default() += 1;
                }
            }
            let mut rows: Vec<DepRow> = dep
                .into_iter()
                .map(|((from_path, to_path), edge_count)| DepRow { from_path, to_path, edge_count })
                .collect();
            rows.sort_by(|a, b| b.edge_count.cmp(&a.edge_count));
            rows.truncate(top_n);
            report.dependencies = Some(rows);
        }

        // ----- Routes -----------------------------------------------------
        if wants(ArchitectureAspect::Routes) {
            let mut rows: Vec<RouteRow> = scoped_nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Route)
                .map(|n| RouteRow {
                    id: n.id.clone(),
                    qualified_name: n.qualified_name.clone(),
                    file_path: n.file_path.clone(),
                    start_line: n.start_line,
                    method: n.extra.get("method").cloned(),
                    path: n.extra.get("path").cloned(),
                })
                .collect();
            rows.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.start_line.cmp(&b.start_line)));
            report.routes = Some(rows);
        }

        // ----- Languages --------------------------------------------------
        if wants(ArchitectureAspect::Languages) {
            let mut node_count: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            let mut files: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> =
                std::collections::BTreeMap::new();
            for n in &scoped_nodes {
                *node_count.entry(n.language.as_str()).or_default() += 1;
                files.entry(n.language.as_str()).or_default().insert(n.file_path.as_str());
            }
            let mut edge_count: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for e in &scoped_edges {
                if let Some(s) = id_to_node.get(e.source.as_str()) {
                    *edge_count.entry(s.language.as_str()).or_default() += 1;
                }
            }
            let rows: Vec<LanguageRow> = node_count
                .into_iter()
                .map(|(language, nc)| LanguageRow {
                    file_count: files.get(language).map(|s| s.len()).unwrap_or(0),
                    edge_count: *edge_count.get(language).unwrap_or(&0),
                    language: language.to_string(),
                    node_count: nc,
                })
                .collect();
            report.languages = Some(rows);
        }

        // ----- Entry points -----------------------------------------------
        if wants(ArchitectureAspect::EntryPoints) {
            let mut entries: Vec<Node> = scoped_nodes
                .iter()
                .filter(|n| is_entry_point(n))
                .cloned()
                .collect();
            entries.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.name.cmp(&b.name)));
            entries.truncate(200);
            report.entry_points = Some(entries);
        }

        // ----- Hotspots ---------------------------------------------------
        if wants(ArchitectureAspect::Hotspots) {
            let mut indeg: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for e in scoped_edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
                *indeg.entry(e.target.as_str()).or_default() += 1;
            }
            let mut rows: Vec<HotspotRow> = indeg
                .into_iter()
                .filter_map(|(id, deg)| id_to_node.get(id).map(|n| (n, deg)))
                .map(|(n, in_degree)| HotspotRow { node: (*n).clone(), in_degree })
                .collect();
            rows.sort_by(|a, b| b.in_degree.cmp(&a.in_degree).then(a.node.qualified_name.cmp(&b.node.qualified_name)));
            rows.truncate(top_n);
            report.hotspots = Some(rows);
        }

        // ----- Boundaries -------------------------------------------------
        if wants(ArchitectureAspect::Boundaries) {
            let mut inbound: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let mut outbound: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for e in scoped_edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
                let (Some(s), Some(t)) =
                    (id_to_node.get(e.source.as_str()), id_to_node.get(e.target.as_str())) else { continue; };
                if s.file_path == t.file_path {
                    continue;
                }
                let sp = package_of(&s.file_path, 2);
                let tp = package_of(&t.file_path, 2);
                if sp != tp {
                    *outbound.entry(sp).or_default() += 1;
                    *inbound.entry(tp).or_default() += 1;
                }
            }
            let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            keys.extend(inbound.keys().cloned());
            keys.extend(outbound.keys().cloned());
            let mut rows: Vec<BoundaryRow> = keys
                .into_iter()
                .map(|path| {
                    let ib = *inbound.get(&path).unwrap_or(&0);
                    let ob = *outbound.get(&path).unwrap_or(&0);
                    let total = (ib + ob) as f64;
                    let ratio = if total > 0.0 { ob as f64 / total } else { 0.0 };
                    BoundaryRow { path, inbound_count: ib, outbound_count: ob, fan_out_ratio: ratio }
                })
                .collect();
            rows.sort_by(|a, b| {
                let ta = a.inbound_count + a.outbound_count;
                let tb = b.inbound_count + b.outbound_count;
                tb.cmp(&ta).then(a.path.cmp(&b.path))
            });
            rows.truncate(top_n);
            report.boundaries = Some(rows);
        }

        // ----- Layers (BFS over CALLS from entry points) ------------------
        if wants(ArchitectureAspect::Layers) {
            let entries: Vec<&Node> = scoped_nodes
                .iter()
                .filter(|n| is_entry_point(n))
                .collect();
            let entry_count = entries.len();
            let mut layer: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            for &seed in &entries {
                layer.insert(seed.id.clone(), 0);
            }
            let mut adj: std::collections::HashMap<&str, Vec<&str>> =
                std::collections::HashMap::new();
            for e in scoped_edges.iter().filter(|e| e.kind == EdgeKind::Calls) {
                adj.entry(e.source.as_str()).or_default().push(e.target.as_str());
            }
            let mut frontier: Vec<&str> = entries.iter().map(|n| n.id.as_str()).collect();
            let mut max_layer = 0u32;
            let mut current_layer = 0u32;
            while !frontier.is_empty() {
                let mut next: Vec<&str> = Vec::new();
                for &node in &frontier {
                    if let Some(targets) = adj.get(node) {
                        for &t in targets {
                            if let Some(t_node) = id_to_node.get(t) {
                                let entry = layer.entry(t_node.id.clone()).or_insert(current_layer + 1);
                                if *entry < current_layer + 1 {
                                    *entry = current_layer + 1;
                                }
                                next.push(t);
                            }
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                current_layer += 1;
                max_layer = current_layer;
                frontier = next;
            }
            let mut per_layer: Vec<usize> = vec![0; (max_layer as usize) + 1];
            for &l in layer.values() {
                if (l as usize) < per_layer.len() {
                    per_layer[l as usize] += 1;
                }
            }
            report.layers = Some(LayersSection {
                entry_count,
                max_layer,
                node_count_per_layer: per_layer,
            });
        }

        // ----- File tree --------------------------------------------------
        if wants(ArchitectureAspect::FileTree) {
            let root = req.path_scope.clone().unwrap_or_default();
            let entries = crate::storage::sqlite::build_file_tree(&scoped_nodes);
            report.file_tree = Some(FileTree { root, entries });
        }

        Ok(report)
    }
}

// ---------------------------------------------------------------------------
// Local helpers for architecture_overview
// ---------------------------------------------------------------------------

/// Look up a node's `file_path` by node id, walking the supplied slice.
/// Returns `None` if the id is unknown (e.g. the edge points to a node
/// outside the loaded scope).
fn id_to_file<'a>(id: &str, nodes: &'a [Node]) -> Option<&'a str> {
    nodes.iter().find(|n| n.id == id).map(|n| n.file_path.as_str())
}

/// Directory prefix of `file_path` up to `depth` segments. Mirrors the
/// SQLite-side helper so both backends bucket identically.
fn package_of(file_path: &str, depth: usize) -> String {
    if file_path.is_empty() {
        return String::new();
    }
    let mut parts: Vec<&str> = file_path.split('/').collect();
    if !parts.is_empty() {
        parts.pop();
    }
    if depth < parts.len() {
        parts.truncate(depth);
    }
    parts.join("/")
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
