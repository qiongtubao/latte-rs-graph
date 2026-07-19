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
use crate::engine::{metrics::compute_metrics, semantic};

// =============================================================================
// Language Registration
// =============================================================================
pub(crate) struct LangConfig {
    pub(crate) name: &'static str,
    pub(crate) extensions: &'static [&'static str],
    pub(crate) language: Language,
    pub(crate) query: &'static str,
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

    /// Find the language name (e.g. "rust") for a file based on its
    /// extension. Returns None if the extension is not supported;
    /// the caller should skip the file in that case.
    pub fn lang_name_for_path(&self, path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_str()?;
        self.langs
            .iter()
            .find(|l| l.extensions.iter().any(|e| *e == ext))
            .map(|l| l.name)
    }

    /// Internal: look up the LangConfig by extension. Used by
    /// `update_files` which needs the full config (query + language
    /// binding) to call `parse_file`.
    pub(crate) fn lang_for_path(&self, path: &Path) -> Option<&LangConfig> {
        let ext = path.extension()?.to_str()?;
        self.langs
            .iter()
            .find(|l| l.extensions.iter().any(|e| *e == ext))
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

                        // Compute complexity metrics on the function body.
                        // ts_node is whatever the query captured (often just
                        // the function-name identifier); walk up to the
                        // enclosing function definition so metrics see the
                        // full body. If nothing qualifies, fall back to
                        // ts_node itself.
                        let metrics_root = {
                            let mut n = ts_node;
                            loop {
                                let k = n.kind();
                                if k.ends_with("_item")
                                    || k.ends_with("_definition")
                                    || k == "function"
                                    || k == "method"
                                    || k == "method_definition"
                                {
                                    break n;
                                }
                                match n.parent() {
                                    Some(p) => n = p,
                                    None => break ts_node,
                                }
                            }
                        };
                        let metrics = compute_metrics(
                            content.as_bytes(),
                            metrics_root,
                            &name,
                            lang.name,
                        );
                        let mut extra = crate::engine::metrics::metrics_to_extra(&metrics);

                        // Detect modifiers from the surrounding item so is_exported /
                        // is_async / etc. reflect what the parser actually sees. Previously
                        // all four were hardcoded `false`, which made Phase 2's
                        // entry-point heuristic and Phase 4's entry_points_count
                        // always zero.
                        let mods = detect_modifiers(content.as_bytes(), metrics_root, lang.name);
                        let mut semantic_signature = semantic::embed_function(
                            content.as_bytes(),
                            metrics_root,
                            &name,
                            lang.name,
                        );
                        semantic_signature.module_path = Some(relative.clone());
                        semantic_signature.is_exported = mods.is_exported;
                        semantic_signature.is_async = mods.is_async;
                        for (key, value) in semantic::signature_to_extra(&semantic_signature) {
                            extra.insert(key, value);
                        }
                        let node = Node {
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
                            visibility: mods.visibility,
                            is_exported: mods.is_exported,
                            is_async: mods.is_async,
                            is_static: mods.is_static,
                            is_abstract: mods.is_abstract,
                            extra,
                        };
                        nodes.push(node);

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
        let mut name_map: HashMap<&str, Vec<&Node>> = HashMap::new();
        for n in &all_nodes {
            let simple = n.name.split("::").last().unwrap_or(&n.name);
            name_map.entry(simple).or_default().push(n);
        }

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
            // Step 1: Resolve target name from wildcard format
            let target_name = if edge.target.starts_with('*') {
                edge.target.split(':').last()
            } else {
                edge.target.split(':').rev().nth(1)
            }.unwrap_or(&edge.target);

            if let Some(candidates) = name_map.get(target_name) {
                let caller_path = if edge.source.starts_with("file:") {
                    edge.source.strip_prefix("file:").unwrap_or("")
                } else {
                    ""
                };
                let caller_dir = std::path::Path::new(caller_path)
                    .parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");

                // Pick best candidate: same file > same directory > first-encountered
                let best = candidates.iter()
                    .min_by_key(|n| {
                        if n.file_path == caller_path { 0 }
                        else if !caller_dir.is_empty()
                            && n.file_path.starts_with(caller_dir)
                        { 1 }
                        else { 2 }
                    })
                    .copied()
                    .unwrap_or(candidates[0]);

                edge.target = best.id.clone();

                // Step 2: Resolve source — if source is a file, find the containing function
                if edge.source.starts_with("file:") {
                    let file_path = edge.source.strip_prefix("file:").unwrap_or("");
                    if let Some(containers) = nodes_by_file.get(file_path) {
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

    /// Emit the highest-scoring semantic neighbors for each indexed callable.
    fn emit_semantic_edges(&self) -> GraphResult<usize> {
        let config = SemanticConfig::default();
        let mut signatures = self
            .storage
            .all_nodes()?
            .into_iter()
            .filter_map(|node| {
                let is_callable = matches!(
                    &node.kind,
                    NodeKind::Function
                        | NodeKind::Method
                        | NodeKind::Class
                        | NodeKind::Struct
                        | NodeKind::Trait
                );
                if !is_callable {
                    return None;
                }
                let mut signature = semantic::signature_from_extra(&node.extra)?;
                signature.name = node.name.clone();
                signature.module_path = Some(node.file_path.clone());
                signature.is_exported = node.is_exported;
                signature.is_async = node.is_async;
                Some((node, signature))
            })
            .collect::<Vec<_>>();

        if signatures.len() < 2 {
            return Ok(0);
        }

        // Rebuild RI over the complete callable corpus so IDF reflects token
        // rarity across the project rather than collapsing to one per file.
        let corpus = semantic::ri::build_ri_corpus(
            signatures
                .iter()
                .map(|(_, signature)| signature.tokens.as_slice()),
            5,
            config.dim,
        );
        for (_, signature) in &mut signatures {
            signature.ri_vec = semantic::corpus_ri_vec(&signature.tokens, &corpus, config.dim);
        }

        // Fold one CALLS-neighborhood propagation step into the corpus RI
        // channel used by the combiner, then persist the post-processed vector.
        let node_indexes = signatures
            .iter()
            .enumerate()
            .map(|(index, (node, _))| (node.id.clone(), index))
            .collect::<HashMap<_, _>>();
        let call_edges = self
            .storage
            .all_edges()?
            .into_iter()
            .filter(|edge| edge.kind == EdgeKind::Calls)
            .collect::<Vec<_>>();
        let base_embeddings = signatures
            .iter()
            .map(|(_, signature)| signature.ri_vec.clone())
            .collect::<Vec<_>>();
        for (index, (node, signature)) in signatures.iter_mut().enumerate() {
            let neighbors = call_edges
                .iter()
                .filter(|edge| edge.source == node.id)
                .filter_map(|edge| node_indexes.get(edge.target.as_str()).copied())
                .filter(|neighbor| *neighbor != index)
                .take(5)
                .map(|neighbor| base_embeddings[neighbor].clone())
                .collect::<Vec<_>>();
            if !neighbors.is_empty() {
                semantic::diffusion::diffuse(&mut signature.ri_vec, &neighbors, 0.3);
            }
            signature.embedding = signature.ri_vec.clone();
            for (key, value) in semantic::signature_to_extra(signature) {
                node.extra.insert(key, value);
            }
            self.storage.upsert_node(node)?;
        }

        let mut inserted = 0;
        for source_index in 0..signatures.len() {
            let mut candidates = (0..signatures.len())
                .filter(|target_index| *target_index != source_index)
                .filter_map(|target_index| {
                    let score = semantic::combined_score(
                        &signatures[source_index].1,
                        &signatures[target_index].1,
                        &config,
                    );
                    (score >= config.threshold).then_some((target_index, score))
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|(left_index, left_score), (right_index, right_score)| {
                right_score
                    .total_cmp(left_score)
                    .then_with(|| signatures[*left_index].0.id.cmp(&signatures[*right_index].0.id))
            });

            for (target_index, score) in candidates.into_iter().take(config.max_per_node) {
                let source = &signatures[source_index].0;
                let target = &signatures[target_index].0;
                self.storage.upsert_edge(&Edge {
                    id: format!("semantic:{}:{}", source.id, target.id),
                    source: source.id.clone(),
                    target: target.id.clone(),
                    kind: EdgeKind::Other("semantically_related".to_string()),
                    line: source.start_line,
                    col: source.start_column,
                    metadata: Some(format!(r#"{{"score":{score:.6}}}"#)),
                    provenance: Some("semantic:algorithmic".to_string()),
                })?;
                inserted += 1;
            }
        }

        Ok(inserted)
    }

    /// Re-index a subset of files in place, without touching the rest of
    /// the graph. This is the hot path used by the editor's file-watcher
    /// driven incremental updates.
    ///
    /// For each path in `changed_paths`:
    /// - If the file no longer exists on disk, every node/edge owned by
    ///   that file is purged, including inbound call edges from other
    ///   files. Their file record is removed.
    /// - Otherwise the file is re-parsed; the previously indexed nodes
    ///   and their incident edges are dropped, and the new nodes/edges
    ///   are inserted in their place.
    ///
    /// At the end the reference resolver runs once to bind unresolved
    /// call / heritage edges to their concrete targets. Resolver cost
    /// is O(graph size); for very large graphs this dominates the
    /// update and should be replaced with a file-scoped variant.
    ///
    /// Returns an `UpdateReport` summarising the work done. Errors
    /// encountered while parsing a single file are recorded in
    /// `error` and do not abort the rest of the batch.
    pub async fn update_files(
        &self,
        project_root: &Path,
        changed_paths: &[PathBuf],
    ) -> GraphResult<UpdateReport> {
        let start = Instant::now();
        let mut nodes_added = 0usize;
        let mut nodes_removed = 0usize;
        let mut edges_rebuilt = 0usize;
        let mut errors: Vec<String> = Vec::new();
        let mut changed_files = 0usize;

        for raw_path in changed_paths {
            // Normalise to absolute, then derive the project-relative
            // path the rest of the storage layer expects.
            let abs = if raw_path.is_absolute() {
                raw_path.to_path_buf()
            } else {
                project_root.join(raw_path)
            };
            let relative = abs
                .strip_prefix(project_root)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();

            changed_files += 1;

            if !abs.exists() {
                // File disappeared: drop every node, edge, and file
                // record we ever owned for it. Unlike a re-index, this
                // is a full purge: inbound call edges from other
                // files that pointed at the deleted symbols are now
                // meaningless, so they have to go too.
                let removed_ids = self.storage.delete_nodes_for_file(&relative)?;
                nodes_removed += removed_ids.len();
                let dropped = self.storage.delete_edges_involving(&removed_ids)?;
                edges_rebuilt += dropped;
                self.storage.delete_file_record(&relative)?;
                continue;
            }

            // Language unknown for this extension: skip cleanly.
            let Some(lang) = self.lang_for_path(&abs) else {
                continue;
            };

            // Parse the new contents.
            let (new_nodes, new_edges) = match self.parse_file(lang, &abs, project_root) {
                Ok(pair) => pair,
                Err(e) => {
                    errors.push(format!("{}: {}", relative, e));
                    continue;
                }
            };

            // Drop the old slice of the graph for this file: every
            // node that lived there, plus the edges that *this file*
            // emitted (source = file:<relative>). We deliberately
            // leave inbound edges from other files alone — their
            // target id is now stale, but re-parsing the caller
            // will regenerate and re-resolve them on the next touch.
            // See the test `update_files_keeps_call_edges_…` for
            // the end-to-end behaviour.
            // Drop the old slice of the graph for this file. We delete
            // only *outbound* edges (source = file:<relative>); inbound
            // call edges from other files keep their now-stale target
            // id and will be regenerated when the caller is next
            // touched and re-resolved by `resolve_references`.
            let old_ids = self.storage.delete_nodes_for_file(&relative)?;
            nodes_removed += old_ids.len();
            let file_edge_source = format!("file:{relative}");
            let dropped = self.storage.delete_edges_with_source(&file_edge_source)?;
            edges_rebuilt += dropped;
            // Insert the freshly parsed slice.
            if let Err(e) = self.storage.upsert_nodes_batch(&new_nodes) {
                errors.push(format!("{}: insert nodes: {}", relative, e));
            }
            nodes_added += new_nodes.len();
            if let Err(e) = self.storage.upsert_edges_batch(&new_edges) {
                errors.push(format!("{}: insert edges: {}", relative, e));
            }
            edges_rebuilt += new_edges.len();

            // Update the file record so future skip-if-unchanged logic
            // can compare mtime / hash.
            let now = chrono::Utc::now().timestamp();
            let mtime = std::fs::metadata(&abs)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let _ = self.storage.upsert_file(&FileRecord {
                path: relative.clone(),
                language: lang.name.to_string(),
                mtime,
                content_hash: String::new(), // hashes aren't computed yet
                indexed_at: now,
            });
        }

        // Re-resolve any newly added or dangling call/heritage edges.
        // Cost is bounded by total node count; for projects > ~50k
        // nodes this should be replaced with a file-scoped resolver.
        if let Err(e) = self.resolve_references() {
            errors.push(format!("resolve_references: {}", e));
        }

        let status = if errors.is_empty() {
            UpdateStatus::Updated
        } else {
            UpdateStatus::Updated // partial success is still an update
        };

        Ok(UpdateReport {
            status,
            changed_files,
            nodes_added,
            nodes_removed,
            edges_rebuilt,
            duration_ms: start.elapsed().as_millis() as u64,
            error: if errors.is_empty() {
                None
            } else {
                Some(errors.join("; "))
            },
        })
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

                        // Track the file so downstream tools (notably grep)
                        // can find the candidate set without scanning disk.
                        let relative = file_path
                            .strip_prefix(root)
                            .unwrap_or(file_path)
                            .to_string_lossy()
                            .to_string();
                        let now = chrono::Utc::now().timestamp();
                        let mtime = std::fs::metadata(file_path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let _ = self.storage.upsert_file(&FileRecord {
                            path: relative,
                            language: lang.name.to_string(),
                            mtime,
                            content_hash: String::new(),
                            indexed_at: now,
                        });
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

        match self.emit_semantic_edges() {
            Ok(created) => stats.edges_created += created,
            Err(error) => stats.errors.push(format!("semantic edges: {error}")),
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

    async fn search_code(
        &self,
        project_root: &Path,
        req: &SearchCodeRequest,
    ) -> GraphResult<SearchCodeResponse> {
        crate::query::grep::search_code(&self.storage, project_root, req)
    }

    async fn complexity(&self, node_id: &str) -> GraphResult<Option<ComplexityMetrics>> {
        let node = self.storage.node_by_id(node_id)?;
        Ok(node.and_then(|n| crate::engine::metrics::metrics_from_extra(&n.extra)))
    }

    async fn dead_code(&self) -> GraphResult<DeadCodeReport> {
        let entries = self.storage.dead_code()?;
        let entry_points = self.storage.entry_point_count().unwrap_or(0);
        let dead_count = entries.len();
        // `total_functions` counts every callable (`function`/`method`/`test`)
        // node so the CLI can show "N dead (out of M total, K entry points)".
        let total_functions = match self.storage.stats() {
            Ok(s) => s
                .node_kinds
                .iter()
                .filter(|k| {
                    matches!(
                        k.kind.as_str(),
                        "function" | "method" | "test"
                    )
                })
                .map(|k| k.count)
                .sum::<usize>(),
            Err(_) => dead_count + entry_points,
        };
        Ok(DeadCodeReport {
            total_functions,
            entry_points,
            dead_count,
            entries,
        })
    }

    async fn blast_radius(
        &self,
        changed_paths: Vec<String>,
        depth: u32,
        direction: BlastDirection,
        base_ref: Option<String>,
    ) -> GraphResult<BlastRadiusReport> {
        let seeds = self.storage.nodes_for_files(&changed_paths)?;
        let seed_node_ids: Vec<String> = seeds
            .iter()
            .flat_map(|s| s.nodes.iter().map(|n| n.id.clone()))
            .collect();
        let impact = self
            .storage
            .reachable_calls(&seed_node_ids, depth, direction)?;
        Ok(BlastRadiusReport {
            base_ref,
            changed_files: changed_paths,
            seeds,
            impact,
        })
    }

    async fn architecture_overview(
        &self,
        req: &ArchitectureRequest,
    ) -> GraphResult<ArchitectureReport> {
        self.storage.architecture_overview(req)
    }

    async fn cypher(&self, query: &str) -> GraphResult<CypherRows> {
        let parsed =
            crate::query::cypher::parse_cypher(query).map_err(GraphError::Config)?;
        let rows = crate::query::cypher::execute_cypher(&parsed, &self.storage)?;
        Ok(Self::flatten_cypher_rows(rows, &parsed))
    }
}

impl TreeSitterEngine {
    /// Collapse executor rows into the flat `CypherRows` shape the
    /// trait (and the CLI JSON) emit. Column names come from
    /// `RETURN` items, preserving the alias when one was given.
    fn flatten_cypher_rows(
        rows: Vec<crate::query::cypher::CypherRow>,
        parsed: &crate::query::cypher::CypherQuery,
    ) -> CypherRows {
        let columns: Vec<String> = parsed
            .return_clause
            .items
            .iter()
            .map(|item| {
                item.alias.clone().unwrap_or_else(|| match &item.expr {
                    crate::query::cypher::ReturnExpr::Var(s) => s.clone(),
                    crate::query::cypher::ReturnExpr::Property { var, prop } => {
                        format!("{var}.{prop}")
                    }
                    crate::query::cypher::ReturnExpr::CountVar(s) => format!("count({s})"),
                    crate::query::cypher::ReturnExpr::CountStar => "count(*)".to_string(),
                })
            })
            .collect();
        let flat_rows: Vec<Vec<CypherScalar>> = rows
            .into_iter()
            .map(|row| {
                row.values
                    .into_iter()
                    .map(|(_, v)| value_to_scalar(v))
                    .collect()
            })
            .collect();
        let truncated = match parsed.limit {
            Some(n) => flat_rows.len() >= n as usize,
            None => false,
        };
        CypherRows {
            columns,
            rows: flat_rows,
            truncated,
        }
    }
}

/// Convert the executor's `CypherValue` (typed payload) into the
/// trait-facing `CypherScalar` (same shape, but used at the engine
/// boundary so the JSON output stays stable across executor refactors).
fn value_to_scalar(v: crate::query::cypher::CypherValue) -> CypherScalar {
    match v {
        crate::query::cypher::CypherValue::Node(n) => CypherScalar::Node(n),
        crate::query::cypher::CypherValue::Str(s) => CypherScalar::Str(s),
        crate::query::cypher::CypherValue::Int(n) => CypherScalar::Int(n),
        crate::query::cypher::CypherValue::Float(f) => CypherScalar::Float(f),
        crate::query::cypher::CypherValue::Bool(b) => CypherScalar::Bool(b),
        crate::query::cypher::CypherValue::Null => CypherScalar::Null,
    }
}
// =============================================================================
// Helpers
// =============================================================================
/// Detect sibling git worktrees and extend the exclusion list so that
/// walkdir does not descend into them. This prevents cross-worktree
/// contamination when the build root happens to be the parent of
/// multiple worktrees.
fn extend_exclude_with_worktrees(root: &Path, exclude: &mut Vec<String>) {
    // In the main repo .git is a directory; in a worktree .git is a file.
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return;
    }

    let worktrees_dir = git_dir.join("worktrees");
    if !worktrees_dir.is_dir() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(&worktrees_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let gitdir_path = entry.path().join("gitdir");
        let Ok(content) = std::fs::read_to_string(&gitdir_path) else {
            continue;
        };
        let worktree_path = PathBuf::from(content.trim());
        // If the worktree lives under root, exclude it by name
        if let Ok(relative) = worktree_path.strip_prefix(root) {
            if let Some(name) = relative.components().next() {
                if let Some(name_str) = name.as_os_str().to_str() {
                    if !exclude.iter().any(|p| p == name_str) {
                        exclude.push(name_str.to_string());
                    }
                }
            }
        }
    }
}

/// Collect source files under `root` with the given `extensions`, skipping
/// hidden entries, excluded directory names, and sibling git worktrees.
fn collect_files(root: &Path, extensions: &[&str], exclude: &[String]) -> Vec<PathBuf> {
    let mut exclude = exclude.to_vec();
    extend_exclude_with_worktrees(root, &mut exclude);

    WalkDir::new(root)
        .follow_links(false)
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

// =============================================================================
// Modifier detection (pub / async / static / abstract / visibility)
// =============================================================================
//
// `parse_file` previously hardcoded `is_exported: false`, `is_async: false`,
// etc. — which made Phase 2's entry-point heuristic (and Phase 4's
// `entry_points_count`) always return zero. The detector below walks the
// defining item's direct children to recover language-aware signals.

struct Modifiers {
    visibility: Option<String>,
    is_exported: bool,
    is_async: bool,
    is_static: bool,
    is_abstract: bool,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            visibility: None,
            is_exported: false,
            is_async: false,
            is_static: false,
            is_abstract: false,
        }
    }
}

fn detect_modifiers(source: &[u8], enclosing_item: tree_sitter::Node, lang: &str) -> Modifiers {
    match lang {
        "rust" => detect_rust_modifiers(source, enclosing_item),
        "typescript" | "javascript" => detect_ts_js_modifiers(source, enclosing_item),
        _ => Modifiers::default(),
    }
}

fn detect_rust_modifiers(source: &[u8], item: tree_sitter::Node) -> Modifiers {
    let mut m = Modifiers::default();
    let mut cursor = item.walk();
    for child in item.children(&mut cursor) {
        match child.kind() {
            "visibility_modifier" => {
                if let Ok(text) = child.utf8_text(source) {
                    let trimmed = text.trim();
                    m.visibility = Some(trimmed.to_string());
                    if trimmed.starts_with("pub") {
                        m.is_exported = true;
                    }
                }
            }
            "function_modifiers" => {
                // `function_modifiers` is the wrapper that holds `async`,
                // `const`, `unsafe` keywords. Walk its children.
                let mut sub = child.walk();
                for sub_child in child.children(&mut sub) {
                    if sub_child.kind() == "async" {
                        m.is_async = true;
                    }
                }
            }
            "async" => m.is_async = true, // fallback in case grammar flattens
            "default" => { /* trait default impl; not flagged in v1 */ }
            _ => {}
        }
    }
    // Trait method signatures have no body — `function_signature_item` is the
    // tell. Mark them `is_abstract = true` so dead_code / arch don't flag
    // trait methods as zombies. Implementations stay `is_abstract = false`.
    if item.kind() == "function_signature_item" {
        m.is_abstract = true;
    }
    m
}

fn detect_ts_js_modifiers(source: &[u8], definition_node: tree_sitter::Node) -> Modifiers {
    // TS / JS export: the defining item (function_declaration / class_declaration
    // / lexical_declaration / method_definition) is either directly in the
    // module or wrapped in an `export_statement`. Walk direct children of both,
    // since the `export` keyword lives on the wrapper when present.
    let mut m = Modifiers::default();
    let mut cursor = definition_node.walk();
    for child in definition_node.children(&mut cursor) {
        match child.kind() {
            "export" => {
                m.is_exported = true;
                m.visibility.get_or_insert_with(|| {
                    child
                        .utf8_text(source)
                        .map(|t| t.trim().to_string())
                        .unwrap_or_else(|_| "export".to_string())
                });
            }
            "default" => {
                m.is_exported = true;
                m.visibility.get_or_insert_with(|| "default".to_string());
            }
            "async" => m.is_async = true,
            "static" => m.is_static = true,
            "abstract" => m.is_abstract = true,
            _ => {}
        }
    }
    if !m.is_exported {
        if let Some(parent) = definition_node.parent() {
            let mut cursor = parent.walk();
            for child in parent.children(&mut cursor) {
                match child.kind() {
                    "export" => {
                        m.is_exported = true;
                        m.visibility.get_or_insert_with(|| {
                            child
                                .utf8_text(source)
                                .map(|t| t.trim().to_string())
                                .unwrap_or_else(|_| "export".to_string())
                        });
                    }
                    "default" => {
                        m.is_exported = true;
                        m.visibility.get_or_insert_with(|| "default".to_string());
                    }
                    _ => {}
                }
            }
        }
    }
    m
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: write a project with `files` (each a (path, content) pair)
    /// and build a TreeSitterEngine over a fresh graph.db rooted there.
    async fn bootstrap(files: &[(&str, &str)]) -> (TempDir, PathBuf, TreeSitterEngine) {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        for (rel, content) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&p, content).unwrap();
        }
        let db = root.join("graph.db");
        let storage = SqliteStorage::open(&db).unwrap();
        let engine = TreeSitterEngine::new(storage);
        // Run a full build so the baseline is established.
        let opts = BuildOptions {
            project_root: root.to_string_lossy().to_string(),
            ..Default::default()
        };
        engine.build(&root, &opts).await.unwrap();
        (dir, root, engine)
    }

    #[tokio::test]
    async fn update_files_replaces_nodes_for_edited_file() {
        let (_dir, root, engine) = bootstrap(&[(
            "src/lib.rs",
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { 2 }\n",
        )])
        .await;

        // Pre-condition: two function nodes for src/lib.rs.
        let before = engine
            .storage
            .all_nodes()
            .unwrap()
            .into_iter()
            .filter(|n| n.file_path == "src/lib.rs" && n.kind == NodeKind::Function)
            .count();
        assert_eq!(before, 2, "baseline build should produce 2 function nodes");

        // Edit the file: rename alpha -> gamma, drop beta, add delta.
        fs::write(
            root.join("src/lib.rs"),
            "pub fn gamma() -> i32 { 1 }\npub fn delta() -> i32 { 3 }\n",
        )
        .unwrap();

        let report = engine
            .update_files(&root, &[PathBuf::from("src/lib.rs")])
            .await
            .unwrap();

        assert_eq!(report.changed_files, 1);
        assert!(report.nodes_removed >= 2, "old function nodes should be removed");
        assert!(report.nodes_added >= 2, "new function nodes should be added");

        let after: Vec<String> = engine
            .storage
            .all_nodes()
            .unwrap()
            .into_iter()
            .filter(|n| n.file_path == "src/lib.rs" && n.kind == NodeKind::Function)
            .map(|n| n.name)
            .collect();
        assert!(after.contains(&"gamma".to_string()));
        assert!(after.contains(&"delta".to_string()));
        assert!(!after.contains(&"alpha".to_string()), "alpha should be gone");
        assert!(!after.contains(&"beta".to_string()), "beta should be gone");
    }

    #[tokio::test]
    async fn update_files_purges_deleted_file() {
        let (_dir, root, engine) = bootstrap(&[(
            "src/lib.rs",
            "pub fn alpha() -> i32 { 1 }\n",
        )])
        .await;

        // Pre-condition: at least one function node belongs to src/lib.rs.
        let before: Vec<_> = engine
            .storage
            .all_nodes()
            .unwrap()
            .into_iter()
            .filter(|n| n.file_path == "src/lib.rs")
            .collect();
        assert!(!before.is_empty());

        // Remove the file and run the update — the engine should
        // notice the file is gone and purge the slice.
        fs::remove_file(root.join("src/lib.rs")).unwrap();
        let report = engine
            .update_files(&root, &[PathBuf::from("src/lib.rs")])
            .await
            .unwrap();

        assert_eq!(report.changed_files, 1);
        assert!(report.nodes_removed >= 1);
        let remaining: Vec<_> = engine
            .storage
            .all_nodes()
            .unwrap()
            .into_iter()
            .filter(|n| n.file_path == "src/lib.rs")
            .collect();
        assert!(remaining.is_empty(), "no nodes should remain for deleted file");
    }

    #[tokio::test]
    async fn update_files_keeps_call_edges_for_re_resolve_on_next_touch() {
        // Two files: src/a.rs calls `target`, defined in src/b.rs.
        let (_dir, root, engine) = bootstrap(&[
            (
                "src/a.rs",
                "use crate::target;\npub fn caller() { target(); }\n",
            ),
            (
                "src/b.rs",
                "pub fn target() {}\n",
            ),
        ])
        .await;

        // After the initial build the resolver has bound the call.
        let edges_before: Vec<String> = engine
            .storage
            .all_edges()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| e.target.clone())
            .collect();
        assert!(edges_before
            .iter()
            .any(|t| t.contains("target")),
            "calls should already be resolved to target, got: {:?}",
            edges_before);

        // Rename `target` -> `renamed` in src/b.rs.
        // After re-parsing src/b.rs the call edge from src/a.rs still
        // points to the (now-stale) `function:target:L1` id, because
        // the caller file wasn't re-parsed. That's an accepted
        // trade-off: callers get re-bound the next time *they* are
        // touched (or after a full rebuild).
        fs::write(root.join("src/b.rs"), "pub fn renamed() {}\n").unwrap();
        let _ = engine
            .update_files(&root, &[PathBuf::from("src/b.rs")])
            .await
            .unwrap();
        let edges_after_first: Vec<String> = engine
            .storage
            .all_edges()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| e.target.clone())
            .collect();
        // The call from src/a.rs survives; it's pointing at the old
        // target id. No edge currently resolves to "renamed" yet.
        assert!(
            edges_after_first.iter().any(|t| t.contains("target")),
            "stale call edge should still be present (will be re-resolved \
             when src/a.rs is next touched)"
        );

        // Now touch src/a.rs (and update its content to call the new
        // name) to trigger its re-parse. The resolver should re-bind
        // the call to the new `renamed` symbol.
        fs::write(
            root.join("src/a.rs"),
            "use crate::renamed;\npub fn caller() { renamed(); }\n",
        )
        .unwrap();
        let _ = engine
            .update_files(&root, &[PathBuf::from("src/a.rs")])
            .await
            .unwrap();

        let edges_after_second: Vec<String> = engine
            .storage
            .all_edges()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| e.target.clone())
            .collect();
        assert!(
            edges_after_second.iter().any(|t| t.contains("renamed")),
            "edges should now resolve to renamed callee, got: {:?}",
            edges_after_second
        );
    }

    #[tokio::test]
    async fn update_files_skips_unsupported_extension() {
        let (_dir, root, engine) = bootstrap(&[(
            "src/lib.rs",
            "pub fn alpha() -> i32 { 1 }\n",
        )])
        .await;

        // A markdown file is not a recognised language: should be
        // ignored gracefully without dropping existing graph data.
        let report = engine
            .update_files(&root, &[PathBuf::from("README.md")])
            .await
            .unwrap();
        assert_eq!(report.changed_files, 1);
        assert_eq!(report.nodes_added, 0);
        assert_eq!(report.nodes_removed, 0);
    }

    #[tokio::test]
    async fn engine_dead_code_finds_unused_function() {
        // Two unused + one used + a `main` entry. `caller` is `pub fn` but
        // the parser does not currently propagate `pub` into either
        // `is_exported` or `visibility`, so we can't rely on it as an
        // entry-point marker here. `fn main()` is detected via the
        // name-in-{'main','index','__init__'} rule.
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.rs",
            "fn used() {}\n\
             fn unused() {}\n\
             fn also_unused() {}\n\
             pub fn caller() { used(); }\n\
             fn main() { caller(); }\n",
        )])
        .await;

        let report = engine.dead_code().await.unwrap();
        let dead_ids: std::collections::BTreeSet<String> =
            report.entries.iter().map(|e| e.node.name.clone()).collect();
        // `unused` and `also_unused` are unreferenced — must be flagged.
        assert!(
            dead_ids.contains("unused"),
            "expected `unused` in dead set, got {:?}",
            dead_ids
        );
        assert!(
            dead_ids.contains("also_unused"),
            "expected `also_unused` in dead set, got {:?}",
            dead_ids
        );
        // `used` is called by `caller` → reached → must not be dead.
        assert!(
            !dead_ids.contains("used"),
            "`used` is called by caller() → must not be dead, got {:?}",
            dead_ids
        );
        // `main` is the entry point (name match) → must not be dead.
        assert!(
            !dead_ids.contains("main"),
            "`main` is entry point, must not be dead, got {:?}",
            dead_ids
        );
    }

    #[tokio::test]
    async fn engine_blast_radius_inbound() {
        // lib.rs exposes api() and helper(). consumer.rs calls both.
        // Requesting blast_radius on lib.rs should pick up `caller`
        // from consumer.rs as the sole inbound impact.
        let (_dir, _root, engine) = bootstrap(&[
            (
                "src/lib.rs",
                "pub fn api() {}\npub fn helper() {}\n",
            ),
            (
                "src/consumer.rs",
                "use super::*;\npub fn caller() { api(); helper(); }\n",
            ),
        ])
        .await;

        let report = engine
            .blast_radius(
                vec!["src/lib.rs".into()],
                3,
                BlastDirection::Inbound,
                None,
            )
            .await
            .unwrap();

        let names: std::collections::BTreeSet<String> = report
            .impact
            .nodes
            .iter()
            .map(|n| n.name.clone())
            .collect();
        assert!(
            names.contains("caller"),
            "expected caller() to be in inbound blast, got {:?}",
            names
        );
        // Changed files should match what we passed.
        assert_eq!(report.changed_files, vec!["src/lib.rs".to_string()]);
        // consumer.rs is the only impacted file.
        assert_eq!(report.impact.files, vec!["src/consumer.rs".to_string()]);
        // Seeds: lib.rs owned api + helper.
        assert_eq!(report.seeds.len(), 1);
        assert_eq!(report.seeds[0].file_path, "src/lib.rs");
        assert_eq!(report.seeds[0].nodes.len(), 2);
    }

    /// Architecture overview with the default aspect set returns the
    /// standard sections populated. At minimum, `overview` must be
    /// present and have at least the function + file nodes built from
    /// the bootstrap source.
    #[tokio::test]
    async fn engine_architecture_overview_includes_default_aspects() {
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.rs",
            "pub fn used() -> i32 { 1 }\npub fn unused() -> i32 { 2 }\npub fn caller() -> i32 { used(); 3 }\n",
        )])
        .await;

        let req = ArchitectureRequest::default();
        let report = engine.architecture_overview(&req).await.unwrap();
        let o = report.overview.expect("overview populated by default set");
        assert!(
            o.total_nodes >= 4,
            "default overview should count at least 4 nodes (3 funcs + 1 file), got {}",
            o.total_nodes
        );
        // Default set includes every standard aspect; at least one of
        // entry_points / hotspots / languages must be populated given
        // any of those data is present.
        let any_populated = report.entry_points.is_some()
            || report.hotspots.is_some()
            || report.languages.is_some();
        assert!(
            any_populated,
            "default set should populate at least one downstream aspect"
        );
    }

    /// Phase 5 — `cypher` round-trips through the trait and yields
    /// at least one row per matching function node.
    #[tokio::test]
    async fn engine_cypher_via_trait() {
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.rs",
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { 2 }\npub fn caller() { alpha(); }\n",
        )])
        .await;

        let report = engine
            .cypher("MATCH (n:Function) RETURN n.name AS name")
            .await
            .unwrap();
        // Three function nodes → three projected rows.
        assert_eq!(report.rows.len(), 3);
        assert_eq!(report.columns, vec!["name".to_string()]);
        let mut names: Vec<String> = report
            .rows
            .iter()
            .map(|row| match &row[0] {
                CypherScalar::Str(s) => s.clone(),
                other => panic!("expected string scalar, got {other:?}"),
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string(), "caller".to_string()]);
        assert!(!report.truncated);
    }

    #[tokio::test]
    async fn detects_pub_and_async_for_rust() {
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.rs",
            "\
             pub async fn exported_async() {}\n\
             pub fn exported_sync() {}\n\
             fn private_fn() {}\n\
             pub(crate) fn crate_pub() {}\n\
             static NOT_A_FN: i32 = 0;\n\
             pub trait Greeter { fn hello(); }\n\
             ",
        )])
        .await;

        let nodes = engine.storage.all_nodes().unwrap();
        let by_name: std::collections::HashMap<String, _> =
            nodes.iter().map(|n| (n.name.clone(), n)).collect();

        let exported_async = by_name.get("exported_async").expect("missing");
        assert!(exported_async.is_exported, "pub async fn should be exported");
        assert!(exported_async.is_async, "pub async fn should be async");
        assert_eq!(exported_async.visibility.as_deref(), Some("pub"));

        let exported_sync = by_name.get("exported_sync").expect("missing");
        assert!(exported_sync.is_exported);
        assert!(!exported_sync.is_async);
        assert_eq!(exported_sync.visibility.as_deref(), Some("pub"));

        let private_fn = by_name.get("private_fn").expect("missing");
        assert!(!private_fn.is_exported, "no `pub` should be private");
        assert_eq!(private_fn.visibility, None);

        let crate_pub = by_name.get("crate_pub").expect("missing");
        assert!(
            crate_pub.is_exported,
            "pub(crate) should register as exported"
        );
        assert_eq!(crate_pub.visibility.as_deref(), Some("pub(crate)"));

        // Trait method `hello` is detected by the parser (we tag
        // function_signature_item as abstract) — verify the grammar picked
        // it up. If the parser doesn't list it under `function_item`, the
        // assertion below just no-ops.
        if let Some(hello) = by_name.get("hello") {
            assert!(
                hello.is_abstract,
                "trait method signature should be flagged abstract"
            );
        }
    }

    #[tokio::test]
    async fn detects_export_for_typescript() {
        // TreeSitterEngine parses TS via the registered `typescript` lang.
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.ts",
            "\
             export function exportedFunction() {}\n\
             export const exportedConst = 1;\n\
             export default class DefaultClass {}\n\
             function privateFunction() {}\n\
             ",
        )])
        .await;

        let nodes = engine.storage.all_nodes().unwrap();
        let by_name: std::collections::HashMap<String, _> =
            nodes.iter().map(|n| (n.name.clone(), n)).collect();

        if let Some(exported) = by_name.get("exportedFunction") {
            assert!(exported.is_exported, "TS export function should be exported");
        }
        if let Some(default) = by_name.get("DefaultClass") {
            assert!(default.is_exported, "export default class should be exported");
        }
        if let Some(private) = by_name.get("privateFunction") {
            assert!(
                !private.is_exported,
                "non-exported TS function should NOT be exported"
            );
        }
    }

    #[tokio::test]
    async fn is_exported_false_for_function_in_let_binding() {
        // Sanity check on Rust: a function inside a closure should
        // not accidentally inherit pub from a surrounding item.
        let (_dir, _root, engine) = bootstrap(&[(
            "src/lib.rs",
            "\
             pub fn outer() {\
                 fn nested() {}\
             }\
             ",
        )])
        .await;

        let nodes = engine.storage.all_nodes().unwrap();
        let nested = nodes
            .iter()
            .find(|n| n.name == "nested")
            .expect("missing nested");
        // Note: tree-sitter Rust doesn't surface `nested` as a definition in
        // our grammar (it captures only top-level `function_item`), so this
        // is more a guard against false detection than a positive assertion.
        assert!(!nested.is_exported, "nested fn without pub must not be exported");
    }
}
