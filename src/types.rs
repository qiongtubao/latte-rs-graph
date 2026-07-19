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
