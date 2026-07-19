use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Node Kinds
// =============================================================================

/// Supported code node kinds.
///
/// Uses strong enum variants + `Other(String)` so adding uncommon kinds
/// does not force a library version bump.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Module,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    EnumMember,
    Function,
    Method,
    Constructor,
    Variable,
    Constant,
    TypeAlias,
    Namespace,
    Import,
    Export,
    Property,
    Field,
    Parameter,
    Test,
    Route,
    Component,
    /// Fallback for languages or grammars that define kinds not in this enum.
    Other(String),
}

impl NodeKind {
    pub fn as_str(&self) -> &str {
        match self {
            NodeKind::File => "file",
            NodeKind::Module => "module",
            NodeKind::Class => "class",
            NodeKind::Struct => "struct",
            NodeKind::Interface => "interface",
            NodeKind::Trait => "trait",
            NodeKind::Enum => "enum",
            NodeKind::EnumMember => "enum_member",
            NodeKind::Function => "function",
            NodeKind::Method => "method",
            NodeKind::Constructor => "constructor",
            NodeKind::Variable => "variable",
            NodeKind::Constant => "constant",
            NodeKind::TypeAlias => "type_alias",
            NodeKind::Namespace => "namespace",
            NodeKind::Import => "import",
            NodeKind::Export => "export",
            NodeKind::Property => "property",
            NodeKind::Field => "field",
            NodeKind::Parameter => "parameter",
            NodeKind::Test => "test",
            NodeKind::Route => "route",
            NodeKind::Component => "component",
            NodeKind::Other(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "file" => NodeKind::File,
            "module" => NodeKind::Module,
            "class" => NodeKind::Class,
            "struct" => NodeKind::Struct,
            "interface" => NodeKind::Interface,
            "trait" => NodeKind::Trait,
            "enum" => NodeKind::Enum,
            "enum_member" => NodeKind::EnumMember,
            "function" => NodeKind::Function,
            "method" => NodeKind::Method,
            "constructor" => NodeKind::Constructor,
            "variable" => NodeKind::Variable,
            "constant" => NodeKind::Constant,
            "type_alias" => NodeKind::TypeAlias,
            "namespace" => NodeKind::Namespace,
            "import" => NodeKind::Import,
            "export" => NodeKind::Export,
            "property" => NodeKind::Property,
            "field" => NodeKind::Field,
            "parameter" => NodeKind::Parameter,
            "test" => NodeKind::Test,
            "route" => NodeKind::Route,
            "component" => NodeKind::Component,
            other => NodeKind::Other(other.to_string()),
        }
    }
}

// =============================================================================
// Edge Kinds
// =============================================================================

/// Supported relationship kinds between nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
    Exports,
    Extends,
    Implements,
    References,
    TypeOf,
    Returns,
    Instantiates,
    Overrides,
    Decorates,
    /// Fallback for uncommon or language-specific edge types.
    Other(String),
}

impl EdgeKind {
    pub fn as_str(&self) -> &str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Exports => "exports",
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::References => "references",
            EdgeKind::TypeOf => "type_of",
            EdgeKind::Returns => "returns",
            EdgeKind::Instantiates => "instantiates",
            EdgeKind::Overrides => "overrides",
            EdgeKind::Decorates => "decorates",
            EdgeKind::Other(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "contains" => EdgeKind::Contains,
            "calls" => EdgeKind::Calls,
            "imports" => EdgeKind::Imports,
            "exports" => EdgeKind::Exports,
            "extends" => EdgeKind::Extends,
            "implements" => EdgeKind::Implements,
            "references" => EdgeKind::References,
            "type_of" => EdgeKind::TypeOf,
            "returns" => EdgeKind::Returns,
            "instantiates" => EdgeKind::Instantiates,
            "overrides" => EdgeKind::Overrides,
            "decorates" => EdgeKind::Decorates,
            other => EdgeKind::Other(other.to_string()),
        }
    }
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            project_root: String::new(),
            graph_dir: None,
            languages: None,
            exclude_patterns: vec![
                "node_modules".into(),
                "target".into(),
                ".git".into(),
                "dist".into(),
                "build".into(),
                "deps".into(),
                "tests".into(),
                "__pycache__".into(),
                ".venv".into(),
                ".codegraph".into(),
                ".latte".into(),
            ],
            concurrency: 4,
            timeout_per_file_ms: 10_000,
        }
    }
}

// =============================================================================
// Core Graph Types
// =============================================================================

/// A node in the code graph representing a code symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_column: u32,
    pub end_column: u32,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub visibility: Option<String>,
    pub is_exported: bool,
    pub is_async: bool,
    pub is_static: bool,
    pub is_abstract: bool,
    pub extra: HashMap<String, String>,
}

/// An edge representing a relationship between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
    pub line: u32,
    pub col: u32,
    pub metadata: Option<String>,
    pub provenance: Option<String>,
}

/// Full graph data returned for visualization / export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphData {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub stats: GraphStats,
}

/// Graph statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub total_files: usize,
    pub node_kinds: Vec<KindCount>,
    pub edge_kinds: Vec<KindCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KindCount {
    pub kind: String,
    pub count: usize,
}

/// Result of a graph build operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildReport {
    pub files_scanned: usize,
    pub nodes_created: usize,
    pub edges_created: usize,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

/// Result of an incremental update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateReport {
    pub status: UpdateStatus,
    pub changed_files: usize,
    pub nodes_added: usize,
    pub nodes_removed: usize,
    pub edges_rebuilt: usize,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateStatus {
    NoChanges,
    Updated,
    Error,
}

/// Result of caller/callee queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationResult {
    pub node: Node,
    pub relations: Vec<RelationItem>,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationItem {
    pub node: Node,
    pub edge: Edge,
    pub depth: u32,
}

/// File record for change detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub language: String,
    pub content_hash: String,
    pub mtime: i64,
    pub indexed_at: i64,
}

/// Detection method for changes.
#[derive(Debug, Clone)]
pub enum ChangeDetection {
    Git,
    MtimeHash,
    Hash,
    Mtime,
}

// =============================================================================
// Configuration
// =============================================================================

/// Options for building a graph.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub project_root: String,
    pub graph_dir: Option<String>,
    pub languages: Option<Vec<String>>,
    pub exclude_patterns: Vec<String>,
    pub concurrency: usize,
    pub timeout_per_file_ms: u64,
}


/// Options for search queries.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub kind_filter: Option<Vec<NodeKind>>,
    pub file_filter: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 50,
            kind_filter: None,
            file_filter: None,
        }
    }
}

// =============================================================================
// Complexity Metrics (computed at parse time, stored in node.extra)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplexityMetrics {
    pub cyclomatic: u32,
    pub cognitive: u32,
    pub max_loop_depth: u32,
    pub alloc_in_loop: bool,
    pub linear_scan_in_loop: bool,
    pub is_recursive: bool,
    pub unguarded_recursion: bool,
}

// === /query/grep types ===
// =============================================================================
// Graph-augmented code search (search_code)
// =============================================================================

/// How to render each hit in the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchCodeMode {
    /// Just the small enclosing node + snippet + count.
    Compact,
    /// Just file paths that contain a hit; no snippet or per-file counts.
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitKind {
    /// Hit landed on a defining node (Function/Method/Class/Struct/Trait/Interface/Route).
    Definition,
    /// Hit landed on a non-test, non-definition node (e.g. inside a function body).
    Usage,
    /// Hit landed on a Test-kind node.
    Test,
}

/// Inputs to `search_code`.
#[derive(Debug, Clone)]
pub struct SearchCodeRequest {
    /// Pattern. `regex=false` → literal substring (v1 only mode).
    pub pattern: String,
    /// Reserved for future. Substring search in v1 even if true.
    pub regex: bool,
    /// If set, only files whose extension matches.
    pub file_extensions: Option<Vec<String>>,
    /// If set, regex must match the file path (anchored match from position 0).
    pub path_filter: Option<String>,
    pub mode: SearchCodeMode,
    pub context_lines: u32,
    pub limit: usize,
}

/// One hit, deduplicated to the enclosing graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodeMatch {
    /// Smallest enclosing definition node, if any. None = hit on top-level (file body).
    pub containing_node: Option<Node>,
    pub file_path: String,
    pub line: u32,
    pub col: u32,
    pub snippet: String,
    /// Raw grep hits inside the containing_node (1 if no enclosing node).
    pub match_count: usize,
    pub hit_kind: HitKind,
    /// Inbound CALLS edges to containing_node. 0 if None. For ranking.
    pub in_degree: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodeResponse {
    pub total_grep_matches: usize,
    pub total_results: usize,
    pub results: Vec<SearchCodeMatch>,
    pub truncated: bool,
}

// =============================================================================
// Dead code + Blast radius analysis
// =============================================================================

/// One dead-code candidate. `reasons_excluded_from_entry` lists the criteria
/// the row matched (so the caller can tell *why* it survived the entry-point
/// filter). Empty when the node had no callers — that's the actual dead case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeEntry {
    pub node: Node,
    pub reasons_excluded_from_entry: Vec<String>,
}

/// Aggregate dead-code report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeReport {
    /// Total callable nodes (`function`/`method`/`test`) currently in the graph.
    pub total_functions: usize,
    /// Subset that matched the entry-point heuristic and therefore were
    /// excluded from `entries`.
    pub entry_points: usize,
    /// `entries.len()` for convenience.
    pub dead_count: usize,
    pub entries: Vec<DeadCodeEntry>,
}

/// One direction of a blast-radius query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlastDirection {
    /// Transitive CALLERS of the changed code (default).
    Inbound,
    /// Transitive CALLEES — what the change depends on.
    Outbound,
    /// Union of inbound + outbound.
    Both,
}

impl Default for BlastDirection {
    fn default() -> Self { Self::Inbound }
}

/// One file plus the definitions (function/class/struct/...) that live in it.
/// Files with zero matching definitions are not returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastSeed {
    pub file_path: String,
    pub nodes: Vec<Node>,
}

/// The transitive reachability set of a blast query, plus a file rollup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastImpact {
    pub nodes: Vec<Node>,
    /// Distinct `file_path` of impacted nodes, sorted.
    pub files: Vec<String>,
    /// `nodes.len()` for convenience.
    pub total_count: usize,
}

/// Aggregate blast-radius report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastRadiusReport {
    pub base_ref: Option<String>,
    pub changed_files: Vec<String>,
    pub seeds: Vec<BlastSeed>,
    pub impact: BlastImpact,
}

// =============================================================================
// FTS5 identifier tokenizer (Phase 3)
// =============================================================================

/// Tokenize an identifier into space-separated lowercase tokens.
///
/// Examples:
///   "updateCloudClient" -> "update cloud client"
///   "parse_user_input" -> "parse user input"
///   "XMLParser_next" -> "xmlparser next"
///   "_leading_underscore" -> "leading underscore"
///   "HTTPRequest" -> "httprequest"
///   "HTTP" -> "http"
///   "URL2Path" -> "url2 path"
///   "" -> ""
///   "___" -> ""
///
/// Algorithm:
///   1. Strip leading `_` chars (silently — they don't become a token).
///   2. Walk char by char. Lowercase / digit always append to the current
///      token. Uppercase starts a NEW token UNLESS the current token ends
///      in an uppercase char (i.e. we're in the middle of an all-caps run,
///      or at the very start). When the current token ends in a lowercase
///      or digit and we see an uppercase, we split. The new uppercase is
///      lowercased if the next char is lowercase (camelCase merge at the
///      upper-before-lower boundary).
///   3. Underscores split tokens.
///   4. Lowercase the joined result.
///   5. Drop empty pieces and adjacent duplicates.
///   6. Join with a single space.
pub fn tokenize_identifier(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    // 1. Drop leading underscores silently.
    let trimmed = input.trim_start_matches('_');
    if trimmed.is_empty() {
        return String::new();
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();

    for i in 0..chars.len() {
        let c = chars[i];
        if c == '_' {
            // Underscore splits the current token.
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else if c.is_ascii_uppercase() {
            // Break only when the current token ends in lowercase or digit
            // (i.e. we just finished a lowercase/digit run).
            let prev_is_lower_or_digit = current
                .chars()
                .last()
                .map(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                .unwrap_or(false);
            if prev_is_lower_or_digit {
                tokens.push(std::mem::take(&mut current));
            }
            // CamelCase merge: if next char is lowercase, lowercase this
            // uppercase too so it joins the new lowercase word.
            let next_is_lower = i + 1 < chars.len() && chars[i + 1].is_ascii_lowercase();
            if next_is_lower {
                current.push(c.to_ascii_lowercase());
            } else {
                current.push(c);
            }
        } else {
            // Lowercase or digit: append to current.
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    // 4. Lowercase the joined result (so "X" -> "x", "HTTP" -> "http").
    let lowered = tokens.join(" ").to_ascii_lowercase();

    // 5. Drop empty pieces and adjacent duplicates.
    let mut prev_emit: Option<&str> = None;
    let mut out_pieces: Vec<&str> = Vec::new();
    for piece in lowered.split_whitespace() {
        if Some(piece) != prev_emit {
            out_pieces.push(piece);
            prev_emit = Some(piece);
        }
    }
    out_pieces.join(" ")
}

/// Convenience: tokenize multiple fields joined by a separator, returning a
/// single contiguous token stream suitable for an FTS5 `body` column.
/// Empty pieces (from blank fields or pure-underscore inputs) are skipped.
pub fn tokenize_for_fts(parts: &[&str]) -> String {
    let mut body = String::new();
    let mut first = true;
    for part in parts {
        let tokens = tokenize_identifier(part);
        if tokens.is_empty() {
            continue;
        }
        if !first {
            body.push(' ');
        }
        body.push_str(&tokens);
        first = false;
    }
    body
}

#[cfg(test)]
mod tokenizer_tests {
    use super::*;

    #[test]
    fn camel() {
        assert_eq!(tokenize_identifier("updateCloudClient"), "update cloud client");
    }

    #[test]
    fn snake() {
        assert_eq!(tokenize_identifier("parse_user_input"), "parse user input");
    }

    #[test]
    fn mixed() {
        assert_eq!(tokenize_identifier("XMLParser_next"), "xmlparser next");
    }

    #[test]
    fn empty() {
        assert_eq!(tokenize_identifier(""), "");
    }

    #[test]
    fn underscore_only() {
        assert_eq!(tokenize_identifier("___"), "");
    }

    #[test]
    fn digits() {
        assert_eq!(tokenize_identifier("foo123Bar"), "foo123 bar");
    }

    #[test]
    fn all_caps() {
        assert_eq!(tokenize_identifier("HTTP"), "http");
    }
}

// =============================================================================
// Architecture overview (Phase 4)
// =============================================================================
//
// A multi-aspect query that returns a structured report about the indexed
// project — file/package rollups, language breakdown, hotspots, layering,
// file tree, etc. Each requested aspect is filled in; aspects not requested
// stay `None` so the report stays slim.

/// Which aspects to include. Empty `aspects` ⇒ "all standard" (no
/// clusters/cycles).
#[derive(Debug, Clone)]
pub struct ArchitectureRequest {
    /// Optional `file_path` prefix to scope analysis (e.g. `"src/foo"`).
    pub path_scope: Option<String>,
    /// Aspects to compute. Empty = use `ArchitectureAspect::default_set()`.
    pub aspects: Vec<ArchitectureAspect>,
    /// Top-N cap for hotspots / boundaries / dependencies.
    pub top_n: usize,
}

impl Default for ArchitectureRequest {
    fn default() -> Self {
        Self {
            path_scope: None,
            aspects: Vec::new(),
            top_n: 20,
        }
    }
}

/// Each aspect is an independent query. `Clusters` and `Cycles` are
/// placeholder variants for the Phase 5 plan — they parse cleanly but the
/// storage layers skip them silently and the CLI prints a heads-up note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
 #[serde(rename_all = "snake_case")]
 pub enum ArchitectureAspect {
    Overview,
    Structure,
    Dependencies,
    Routes,
    Languages,
    Packages,
    EntryPoints,
    Hotspots,
    Boundaries,
    Layers,
    FileTree,
    /// Unsupported in v1 (needs Leiden / Phase 5).
    Clusters,
    /// Unsupported in v1 (needs SCC).
    Cycles,
}

impl ArchitectureAspect {
    /// Parse a comma-separated list of aspect names. Accepts kebab- or
    /// snake-case. Unknown values are dropped silently so the CLI can take
    /// free-form user input.
    pub fn parse_list(s: &str) -> Vec<Self> {
        s.split(',')
            .map(|t| t.trim().to_ascii_lowercase().replace('-', "_"))
            .filter_map(|t| match t.as_str() {
                "overview" => Some(Self::Overview),
                "structure" => Some(Self::Structure),
                "dependencies" | "deps" => Some(Self::Dependencies),
                "routes" => Some(Self::Routes),
                "languages" | "langs" => Some(Self::Languages),
                "packages" | "pkgs" => Some(Self::Packages),
                "entry_points" => Some(Self::EntryPoints),
                "hotspots" => Some(Self::Hotspots),
                "boundaries" => Some(Self::Boundaries),
                "layers" => Some(Self::Layers),
                "file_tree" | "filetree" => Some(Self::FileTree),
                "clusters" => Some(Self::Clusters),
                "cycles" => Some(Self::Cycles),
                _ => None,
            })
            .collect()
    }

    /// The default set: everything except the deferred `Clusters`/`Cycles`.
    pub fn default_set() -> Vec<Self> {
        vec![
            Self::Overview,
            Self::Structure,
            Self::Dependencies,
            Self::Routes,
            Self::Languages,
            Self::Packages,
            Self::EntryPoints,
            Self::Hotspots,
            Self::Boundaries,
            Self::Layers,
            Self::FileTree,
        ]
    }
}

/// All aspect data assembled for one query. Each field that wasn't
/// requested stays `None` so the report stays slim and JSON shape stays
/// predictable across runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArchitectureReport {
    pub overview: Option<OverviewSection>,
    pub structure: Option<Vec<PackageRow>>,
    pub dependencies: Option<Vec<DepRow>>,
    pub routes: Option<Vec<RouteRow>>,
    pub languages: Option<Vec<LanguageRow>>,
    pub packages: Option<Vec<PackageRow>>,
    pub entry_points: Option<Vec<Node>>,
    pub hotspots: Option<Vec<HotspotRow>>,
    pub boundaries: Option<Vec<BoundaryRow>>,
    pub layers: Option<LayersSection>,
    pub file_tree: Option<FileTree>,
    pub requested_aspects: Vec<ArchitectureAspect>,
    pub path_scope: Option<String>,
}

/// Headline counts rolled up across the whole graph (or the path scope).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewSection {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub total_files: usize,
    pub total_routes: usize,
    pub languages_count: usize,
    pub entry_points_count: usize,
    pub dead_count: usize,
    pub call_edges_count: usize,
}

/// One row of the `Structure`/`Packages` rollup — a directory prefix
/// (depth-2 by default) and the node count observed under it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageRow {
    pub path: String,
    pub node_count: usize,
    pub edge_count: usize,
}

/// One row of the `Dependencies` rollup — outgoing imports between two
/// distinct files, grouped by (from_path, to_path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepRow {
    pub from_path: String,
    pub to_path: String,
    pub edge_count: usize,
}

/// One row of the `Routes` aspect. `method`/`path` are pulled from
/// `node.extra` when the parser populated them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRow {
    pub id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub method: Option<String>,
    pub path: Option<String>,
}

/// One row of the `Languages` aspect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageRow {
    pub language: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub file_count: usize,
}

/// One row of the `Hotspots` aspect — a node and its inbound CALLS degree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotspotRow {
    pub node: Node,
    pub in_degree: usize,
}

/// One row of the `Boundaries` aspect — a directory with high fan-in and
/// fan-out across package boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryRow {
    pub path: String,
    pub inbound_count: usize,
    pub outbound_count: usize,
    pub fan_out_ratio: f64,
}

/// `Layers` aspect result — a BFS layering from entry points along CALLS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayersSection {
    pub entry_count: usize,
    pub max_layer: u32,
    pub node_count_per_layer: Vec<usize>,
}

/// `FileTree` aspect — a nested directory tree from distinct file paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTree {
    pub root: String,
    pub entries: Vec<FileTreeEntry>,
}

/// A single node in the file tree (either a directory or a leaf file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTreeEntry {
    pub name: String,
    pub kind: String,
    pub children: Vec<FileTreeEntry>,
}
