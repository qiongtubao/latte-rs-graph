use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::error::{GraphError, GraphResult};
use std::time::Instant;
use walkdir::WalkDir;
use uuid::Uuid;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};
use crate::storage::SqliteStorage;
use crate::traits::GraphProvider;
use crate::types::*;

// =============================================================================
// Language Registration
// =============================================================================

struct LangConfig {
    name: &'static str,
    extensions: &'static [&'static str],
    language: Language,
    query: &'static str,
}

fn registered_languages() -> Vec<LangConfig> {
    vec![
        LangConfig {
            name: "rust",
            extensions: &["rs"],
            language: tree_sitter_rust::LANGUAGE.into(),
            query: include_str!("../../grammars/rust.scm"),
        },
        LangConfig {
            name: "typescript",
            extensions: &["ts", "tsx"],
            language: tree_sitter_typescript::LANGUAGE_TSX.into(),
            query: include_str!("../../grammars/typescript.scm"),
        },
        LangConfig {
            name: "javascript",
            extensions: &["js", "jsx", "mjs", "cjs"],
            language: tree_sitter_javascript::LANGUAGE.into(),
            query: include_str!("../../grammars/typescript.scm"),
        },
        LangConfig {
            name: "c",
            extensions: &["c", "h"],
            language: tree_sitter_c::LANGUAGE.into(),
            query: include_str!("../../grammars/c.scm"),
        },
        LangConfig {
            name: "cpp",
            extensions: &["cpp", "hpp", "cc", "hh", "cxx", "hxx"],
            language: tree_sitter_cpp::LANGUAGE.into(),
            query: include_str!("../../grammars/c.scm"),
        },
    ]
}

// =============================================================================
// TreeSitterEngine
// =============================================================================

pub struct TreeSitterEngine {
    storage: SqliteStorage,
    langs: Vec<LangConfig>,
}


impl TreeSitterEngine {
    pub fn new(storage: SqliteStorage) -> Self {
        Self {
            storage,
            langs: registered_languages(),
        }
    }

    /// Parse a single source file, returning its nodes and edges.
    fn parse_file(
        &self,
        lang: &LangConfig,
        file_path: &Path,
        project_root: &Path,
    ) -> GraphResult<(Vec<Node>, Vec<Edge>)> {
        let content = std::fs::read_to_string(file_path).map_err(|e| GraphError::ParseError {
            path: file_path.to_path_buf(),
            message: format!("Cannot read file: {}", e),
        })?;

        if content.len() > 5_000_000 {
            return Ok((vec![], vec![]));
        }

        let relative = file_path
            .strip_prefix(project_root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let mut parser = Parser::new();
        parser.set_language(&lang.language).map_err(|e| {
            GraphError::TreeSitter(format!("Cannot set language: {}", e))
        })?;

        let tree = parser.parse(&content, None).ok_or_else(|| {
            GraphError::ParseError {
                path: file_path.to_path_buf(),
                message: "Tree-sitter parse returned None".into(),
            }
        })?;

        let root = tree.root_node();
        let query =
            Query::new(&lang.language, lang.query).map_err(|e| {
                GraphError::TreeSitter(format!("Query error: {}", e))
            })?;

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, content.as_bytes());

        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();

        // File node
        let file_node_id = format!("file:{}", relative);
        nodes.push(Node {
            id: file_node_id.clone(),
            kind: NodeKind::File,
            name: relative.clone(),
            qualified_name: relative.clone(),
            file_path: relative.clone(),
            language: lang.name.to_string(),
            start_line: 0,
            end_line: 0,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
            is_static: false,
            is_abstract: false,
            extra: HashMap::new(),
        });

        while let Some(m) = matches.next() {
            for capture in m.captures.iter() {
                let capture_name = query.capture_names()[capture.index as usize];
                let ts_node = capture.node;
                let start = ts_node.start_position();
                let end = ts_node.end_position();
                let text = ts_node.utf8_text(content.as_bytes()).unwrap_or("");

                match capture_name {
                    "definition" => {
                        let mut kind = NodeKind::Function;
                        let mut is_test = false;

                        for prop in query.property_settings(m.pattern_index) {
                            if &*prop.key == "kind" {
                                if let Some(val) = &prop.value {
                                    kind = NodeKind::from_str(val);
                                }
                            }
                        }

                        let name = text.to_string();
                        let qualified = format!(
                            "{}::{}",
                            relative.replace('/', "::").replace('.', ""),
                            name
                        );

                        if kind == NodeKind::Function
                            && (name.starts_with("test_")
                                || name.starts_with("it_")
                                || name.contains("_test"))
                        {
                            is_test = true;
                        }

                        // Get the full definition range by walking up to the definition parent
                        let (full_start, full_end) = {
                            let mut n = ts_node;
                            // Walk up to get the outermost definition node (function_item, function_definition, etc.)
                            while let Some(parent) = n.parent() {
                                let kind = parent.kind();
                                // Stop at top-level constructs: function, struct, enum, impl, class, interface
                                if kind.ends_with("_definition") || kind.ends_with("_item")
                                    || kind == "struct_specifier" || kind == "enum_specifier"
                                {
                                    n = parent;
                                    break;
                                }
                                // If parent is root, use current node
                                if parent.parent().is_none() { break; }
                                n = parent;
                            }
                            (n.start_position(), n.end_position())
                        };

                        let def_kind = if is_test { NodeKind::Test } else { kind.clone() };
                        let node_id = format!("{}:{}:L{}", kind.as_str(), name, start.row + 1);
                        let sig = extract_signature(&content, start.row, full_start.row);

                        nodes.push(Node {
                            id: node_id.clone(),
                            kind: def_kind,
                            name,
                            qualified_name: qualified,
                            file_path: relative.clone(),
                            language: lang.name.to_string(),
                            start_line: full_start.row as u32 + 1,
                            end_line: full_end.row as u32 + 1,
                            start_column: full_start.column as u32,
                            end_column: full_end.column as u32,
                            signature: sig,
                            docstring: None,
                            visibility: None,
                            is_exported: false,
                            is_async: false,
                            is_static: false,
                            is_abstract: false,
                            extra: HashMap::new(),
                        });

                        edges.push(Edge {
                            id: Uuid::new_v4().to_string(),
                            source: file_node_id.clone(),
                            target: node_id,
                            kind: EdgeKind::Contains,
                            line: start.row as u32 + 1,
                            col: start.column as u32,
                            metadata: None,
                            provenance: Some("tree-sitter".into()),
                        });
                    }

                    "call" => {
                        let call_name = text.to_string();
                        for prop in query.property_settings(m.pattern_index) {
                            if &*prop.key == "call-name" {
                                edges.push(Edge {
                                    id: Uuid::new_v4().to_string(),
                                    source: file_node_id.clone(),
                                    target: format!("function:{}:*", call_name),
                                    kind: EdgeKind::Calls,
                                    line: start.row as u32 + 1,
                                    col: start.column as u32,
                                    metadata: Some(format!(
                                        r#"{{"name":"{}","pattern":"{}"}}"#,
                                        call_name, prop.value.as_deref().unwrap_or("direct")
                                    )),
                                    provenance: Some("tree-sitter".into()),
                                });
                            }
                        }
                    }

                    "import" => {
                        let import_name = text.to_string();
                        let import_id =
                            format!("import:{}:L{}", import_name, start.row + 1);

                        nodes.push(Node {
                            id: import_id.clone(),
                            kind: NodeKind::Import,
                            name: import_name.clone(),
                            qualified_name: import_name,
                            file_path: relative.clone(),
                            language: lang.name.to_string(),
                            start_line: start.row as u32 + 1,
                            end_line: start.row as u32 + 1,
                            start_column: start.column as u32,
                            end_column: end.column as u32,
                            signature: None,
                            docstring: None,
                            visibility: None,
                            is_exported: false,
                            is_async: false,
                            is_static: false,
                            is_abstract: false,
                            extra: HashMap::new(),
                        });

                        edges.push(Edge {
                            id: Uuid::new_v4().to_string(),
                            source: file_node_id.clone(),
                            target: import_id,
                            kind: EdgeKind::Imports,
                            line: start.row as u32 + 1,
                            col: start.column as u32,
                            metadata: None,
                            provenance: Some("tree-sitter".into()),
                        });
                    }

                    "heritage" => {
                        let heritage_name = text.to_string();
                        let mut heritage_kind = "extends";

                        for prop in query.property_settings(m.pattern_index) {
                            if &*prop.key == "heritage-kind" {
                                if let Some(val) = &prop.value {
                                    heritage_kind = val;
                                }
                            }
                        }

                        let edge_kind = match heritage_kind {
                            "implements" => EdgeKind::Implements,
                            _ => EdgeKind::Extends,
                        };

                        edges.push(Edge {
                            id: Uuid::new_v4().to_string(),
                            source: file_node_id.clone(),
                            target: format!("*:{}", heritage_name),
                            kind: edge_kind,
                            line: start.row as u32 + 1,
                            col: start.column as u32,
                            metadata: None,
                            provenance: Some("tree-sitter".into()),
                        });
                    }

                    _ => {}
                }
            }
        }

        Ok((nodes, edges))
    }

    /// Resolve unresolved call/import/heritage targets.
    ///
    /// Two steps:
    /// 1. Resolve target: `function:main:*` → actual node ID `function:main:L6913`
    /// 2. Resolve source: if source is a file, find which function node contains
    ///    the call by checking line ranges.
    fn resolve_references(&self) -> GraphResult<()> {
        let all_nodes = self.storage.all_nodes()?;
        let name_map: HashMap<&str, &Node> = all_nodes.iter().filter_map(|n| {
            let simple = n.name.split("::").last().unwrap_or(&n.name);
            Some((simple, n))
        })
        .collect();

        // Index nodes by file for container resolution
        let mut nodes_by_file: HashMap<&str, Vec<&Node>> = HashMap::new();
        for n in &all_nodes {
            if n.kind == NodeKind::File { continue; }
            nodes_by_file.entry(n.file_path.as_str()).or_default().push(n);
        }

        let all_edges = self.storage.all_edges()?;
        let unresolved: Vec<Edge> = all_edges
            .into_iter()
            .filter(|e| e.target.contains(":*") || e.target.starts_with('*'))
            .collect();

        for mut edge in unresolved {
            // Step 1: Resolve target
            let target_name = if edge.target.starts_with('*') {
                edge.target.split(':').last()
            } else {
                edge.target.split(':').rev().nth(1)
            }.unwrap_or(&edge.target);

            if let Some(target_node) = name_map.get(target_name) {
                edge.target = target_node.id.clone();

                // Step 2: Resolve source — if source is a file, find the containing function
                if edge.source.starts_with("file:") {
                    // Extract the file path
                    let file_path = edge.source.strip_prefix("file:").unwrap_or("");
                    if let Some(containers) = nodes_by_file.get(file_path) {
                        // Find the function that contains the call line
                        for func_node in containers {
                            if func_node.kind != NodeKind::Function
                                && func_node.kind != NodeKind::Method
                                && func_node.kind != NodeKind::Test
                            {
                                continue;
                            }
                            if edge.line >= func_node.start_line && edge.line <= func_node.end_line {
                                edge.source = func_node.id.clone();
                                break;
                            }
                        }
                    }
                }

                self.storage.upsert_edge(&edge)?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl GraphProvider for TreeSitterEngine {
    fn name(&self) -> &str {
        "tree-sitter"
    }

    fn supported_languages(&self) -> Vec<&str> {
        self.langs.iter().map(|l| l.name).collect()
    }

    async fn build(&self, root: &Path, options: &BuildOptions) -> GraphResult<BuildReport> {
        let start = Instant::now();
        self.storage.clear_all()?;

        let mut stats = BuildReport {
            files_scanned: 0,
            nodes_created: 0,
            edges_created: 0,
            errors: vec![],
            duration_ms: 0,
        };

        for lang in &self.langs {
            let files = collect_files(root, lang.extensions, &options.exclude_patterns);

            for file_path in &files {
                match self.parse_file(lang, file_path, root) {
                    Ok((nodes, edges)) => {
                        stats.files_scanned += 1;
                        stats.nodes_created += nodes.len();
                        stats.edges_created += edges.len();

                        if let Err(e) = self.storage.upsert_nodes_batch(&nodes) {
                            stats.errors.push(format!(
                                "DB insert nodes {}: {}",
                                file_path.display(),
                                e
                            ));
                        }
                        if let Err(e) = self.storage.upsert_edges_batch(&edges) {
                            stats.errors.push(format!(
                                "DB insert edges {}: {}",
                                file_path.display(),
                                e
                            ));
                        }
                    }
                    Err(e) => {
                        stats
                            .errors
                            .push(format!("Parse {}: {}", file_path.display(), e));
                    }
                }
            }
        }

        if let Err(e) = self.resolve_references() {
            stats.errors.push(format!("Reference resolution: {}", e));
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    async fn update(&self, root: &Path) -> GraphResult<UpdateReport> {
        let report = self.build(root, &BuildOptions::default()).await?;
        Ok(UpdateReport {
            status: UpdateStatus::Updated,
            changed_files: report.files_scanned,
            nodes_added: report.nodes_created,
            nodes_removed: 0,
            edges_rebuilt: report.edges_created,
            duration_ms: report.duration_ms,
            error: None,
        })
    }

    async fn graph_data(&self) -> GraphResult<GraphData> {
        let nodes = self.storage.all_nodes()?;
        let edges = self.storage.all_edges()?;
        let stats = self.storage.stats()?;
        Ok(GraphData { nodes, edges, stats })
    }

    async fn search(&self, query: &str, options: &SearchOptions) -> GraphResult<Vec<Node>> {
        self.storage.search_nodes(query, options.limit)
    }

    async fn find_definitions(&self, name: &str) -> GraphResult<Vec<Node>> {
        self.storage.find_definitions(name)
    }

    async fn node_by_id(&self, id: &str) -> GraphResult<Option<Node>> {
        self.storage.node_by_id(id)
    }

    async fn subgraph(&self, node_id: &str, depth: u32) -> GraphResult<GraphData> {
        let mut visited_nodes = std::collections::HashSet::new();
        let mut visited_edges = std::collections::HashSet::new();
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        let mut queue: Vec<(String, u32)> = vec![(node_id.to_string(), 0)];
        visited_nodes.insert(node_id.to_string());

        while let Some((current_id, current_depth)) = queue.pop() {
            if current_depth > depth {
                continue;
            }

            if let Some(node) = self.storage.node_by_id(&current_id)? {
                nodes.push(node);
            }

            for edge in self.storage.edges_for_node(&current_id)? {
                let target = edge.target.clone();
                let source = edge.source.clone();
                if visited_edges.insert(edge.id.clone()) {
                    edges.push(edge);
                }

                let neighbor = if source == current_id {
                    target
                } else {
                    source
                };

                if visited_nodes.insert(neighbor.clone()) && current_depth < depth {
                    queue.push((neighbor, current_depth + 1));
                }
            }
        }

        let stats = self.storage.stats()?;
        Ok(GraphData { nodes, edges, stats })
    }

    async fn stats(&self) -> GraphResult<GraphStats> {
        self.storage.stats()
    }

    async fn callers(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult> {
        let node = match self.storage.node_by_id(node_id)? {
            Some(n) => n,
            None => return Err(GraphError::NotFound(format!("Node {} not found", node_id))),
        };

        let mut relations = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: Vec<(String, u32)> = vec![(node_id.to_string(), 0)];
        visited.insert(node_id.to_string());

        while let Some((current_id, current_depth)) = queue.pop() {
            if current_depth >= depth {
                continue;
            }

            for edge in self.storage.edges_for_node(&current_id)? {
                if edge.target == current_id {
                    if let Some(caller_node) = self.storage.node_by_id(&edge.source)? {
                        relations.push(RelationItem {
                            node: caller_node.clone(),
                            edge: edge.clone(),
                            depth: current_depth + 1,
                        });

                        if visited.insert(caller_node.id.clone()) && current_depth + 1 < depth {
                            queue.push((caller_node.id.clone(), current_depth + 1));
                        }
                    }
                }
            }
        }

        Ok(RelationResult { node, relations, depth })
    }

    async fn callees(&self, node_id: &str, depth: u32) -> GraphResult<RelationResult> {
        let node = match self.storage.node_by_id(node_id)? {
            Some(n) => n,
            None => return Err(GraphError::NotFound(format!("Node {} not found", node_id))),
        };

        let mut relations = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: Vec<(String, u32)> = vec![(node_id.to_string(), 0)];
        visited.insert(node_id.to_string());

        while let Some((current_id, current_depth)) = queue.pop() {
            if current_depth >= depth {
                continue;
            }

            for edge in self.storage.edges_for_node(&current_id)? {
                if edge.source == current_id {
                    if let Some(callee_node) = self.storage.node_by_id(&edge.target)? {
                        relations.push(RelationItem {
                            node: callee_node.clone(),
                            edge: edge.clone(),
                            depth: current_depth + 1,
                        });

                        if visited.insert(callee_node.id.clone()) && current_depth + 1 < depth {
                            queue.push((callee_node.id.clone(), current_depth + 1));
                        }
                    }
                }
            }
        }

        Ok(RelationResult { node, relations, depth })
    }

    async fn clear(&self) -> GraphResult<()> {
        self.storage.clear_all()
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn collect_files(root: &Path, extensions: &[&str], exclude: &[String]) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.depth() > 0 && name.starts_with('.') {
                return false;
            }
            if e.file_type().is_dir() {
                return !exclude.iter().any(|p| name == *p);
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| extensions.contains(&ext))
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn extract_signature(content: &str, start_row: usize, _end_row: usize) -> Option<String> {
    let line = content.lines().nth(start_row)?;
    let sig = line.trim().chars().take(120).collect::<String>();
    if sig.is_empty() {
        None
    } else {
        Some(sig)
    }
}
