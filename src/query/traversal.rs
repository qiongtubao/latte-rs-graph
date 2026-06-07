use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::GraphResult;
use crate::types::*;

/// BFS traversal of the graph from a set of start nodes.
pub fn bfs(
    nodes: &[Node],
    edges: &[Edge],
    start_ids: &[String],
    max_depth: u32,
) -> GraphResult<GraphData> {
    let node_map: HashMap<String, &Node> =
        nodes.iter().map(|n| (n.id.clone(), n)).collect();
    let mut adj: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for edge in edges {
        adj.entry(edge.source.as_str()).or_default().push(edge);
        adj.entry(edge.target.as_str()).or_default().push(edge);
    }

    let mut visited_nodes: HashSet<&str> = HashSet::new();
    let mut visited_edges: HashSet<&str> = HashSet::new();
    let mut result_nodes: Vec<Node> = Vec::new();
    let mut result_edges: Vec<Edge> = Vec::new();

    let mut queue: VecDeque<(&str, u32)> = VecDeque::new();
    for id in start_ids {
        if let Some(n) = node_map.get(id.as_str()) {
            visited_nodes.insert(n.id.as_str());
            queue.push_back((n.id.as_str(), 0));
            result_nodes.push((*n).clone());
        }
    }

    while let Some((current_id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        if let Some(neighbors) = adj.get(current_id) {
            for edge in neighbors {
                if visited_edges.insert(edge.id.as_str()) {
                    result_edges.push((*edge).clone());
                }

                let next = if edge.source == current_id {
                    &edge.target
                } else {
                    &edge.source
                };

                if visited_nodes.insert(next) {
                    if let Some(n) = node_map.get(next) {
                        result_nodes.push((*n).clone());
                    }
                    queue.push_back((next, depth + 1));
                }
            }
        }
    }

    let nn = result_nodes.len();
    let ne = result_edges.len();
    let kind_counts = count_kinds(&result_nodes);
    let edge_kind_counts = count_edge_kinds(&result_edges);

    Ok(GraphData {
        nodes: result_nodes,
        edges: result_edges,
        stats: GraphStats {
            total_nodes: nn,
            total_edges: ne,
            total_files: 0,
            node_kinds: kind_counts,
            edge_kinds: edge_kind_counts,
        },
    })
}

/// Find all nodes that call into (or are called by) a given node.
pub fn impact_analysis(
    nodes: &[Node],
    edges: &[Edge],
    node_id: &str,
    depth: u32,
) -> GraphResult<ImpactAnalysis> {
    let edges_by_target: HashMap<&str, Vec<&Edge>> = edges
        .iter()
        .fold(HashMap::new(), |mut acc, e| {
            acc.entry(e.target.as_str()).or_default().push(e);
            acc
        });
    let edges_by_source: HashMap<&str, Vec<&Edge>> = edges
        .iter()
        .fold(HashMap::new(), |mut acc, e| {
            acc.entry(e.source.as_str()).or_default().push(e);
            acc
        });

    let node_map: HashMap<&str, &Node> =
        nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Collect callers (reverse edges)
    let mut callers: Vec<(&Node, u32)> = Vec::new();
    let mut visited = HashSet::new();
    let mut queue: VecDeque<(&str, u32)> = VecDeque::new();
    queue.push_back((node_id, 0));
    visited.insert(node_id);

    while let Some((current, d)) = queue.pop_front() {
        if d >= depth {
            break;
        }
        if let Some(incoming) = edges_by_target.get(current) {
            for edge in incoming {
                if visited.insert(edge.source.as_str()) {
                    if let Some(n) = node_map.get(edge.source.as_str()) {
                        callers.push((n, d + 1));
                        queue.push_back((edge.source.as_str(), d + 1));
                    }
                }
            }
        }
    }

    // Collect callees (forward edges)
    let mut callees: Vec<(&Node, u32)> = Vec::new();
    visited.clear();
    queue.clear();
    queue.push_back((node_id, 0));
    visited.insert(node_id);

    while let Some((current, d)) = queue.pop_front() {
        if d >= depth {
            break;
        }
        if let Some(outgoing) = edges_by_source.get(current) {
            for edge in outgoing {
                if visited.insert(edge.target.as_str()) {
                    if let Some(n) = node_map.get(edge.target.as_str()) {
                        callees.push((n, d + 1));
                        queue.push_back((edge.target.as_str(), d + 1));
                    }
                }
            }
        }
    }

    let node = node_map.get(node_id).map(|n| (*n).clone());

    Ok(ImpactAnalysis {
        node,
        callers: callers.into_iter().map(|(n, d)| ((*n).clone(), d)).collect(),
        callees: callees.into_iter().map(|(n, d)| ((*n).clone(), d)).collect(),
        depth,
    })
}

#[derive(Debug, Clone)]
pub struct ImpactAnalysis {
    pub node: Option<Node>,
    pub callers: Vec<(Node, u32)>,
    pub callees: Vec<(Node, u32)>,
    pub depth: u32,
}

fn count_kinds(nodes: &[Node]) -> Vec<KindCount> {
    let mut map: HashMap<String, usize> = HashMap::new();
    for n in nodes {
        *map.entry(n.kind.as_str().to_string()).or_default() += 1;
    }
    let mut v: Vec<KindCount> = map
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v
}

fn count_edge_kinds(edges: &[Edge]) -> Vec<KindCount> {
    let mut map: HashMap<String, usize> = HashMap::new();
    for e in edges {
        *map.entry(e.kind.as_str().to_string()).or_default() += 1;
    }
    let mut v: Vec<KindCount> = map
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v
}
