//! Cypher AST (Phase 5 v1 subset).
//!
//! Defines the parsed shape of a query. No execution logic lives here — see
//! `planner.rs` (SQL/relational translation) and `executor.rs` (storage
//! traversal).

use serde::{Deserialize, Serialize};

// =============================================================================
// Top-level query
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CypherQuery {
    pub match_clause: MatchClause,
    pub where_clause: Option<Expr>,
    pub return_clause: ReturnClause,
    pub order_by: Option<OrderBy>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchClause {
    pub pattern: Pattern,
}

// =============================================================================
// Patterns
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    SingleNode(NodePattern),
    DirectedEdge {
        from: NodePattern,
        edge: EdgePattern,
        to: NodePattern,
    },
    VariableLength {
        from: NodePattern,
        edge: EdgePattern,
        min_hops: u32,
        max_hops: u32,
        to: NodePattern,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePattern {
    pub var: String,
    /// Capitalised kind, e.g. `Function` → `NodeKind::Function`.
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgePattern {
    pub var: Option<String>,
    pub kind: Option<String>,
}

// =============================================================================
// Expressions
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Var(String),
    Property { var: String, prop: String },
    Literal(Literal),
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    InList {
        var: String,
        prop: String,
        values: Vec<Literal>,
    },
    StartsWith {
        var: String,
        prop: String,
        value: Literal,
    },
    Contains {
        var: String,
        prop: String,
        value: Literal,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

// =============================================================================
// RETURN / ORDER BY
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnClause {
    pub items: Vec<ReturnItem>,
    /// Parse-only in v1 — `RETURN DISTINCT` is accepted but not enforced
    /// during execution. Tracked so the parser round-trips through the
    /// executor without losing intent.
    pub distinct: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnItem {
    pub expr: ReturnExpr,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReturnExpr {
    Var(String),
    Property { var: String, prop: String },
    CountVar(String),
    CountStar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBy {
    pub var: String,
    pub prop: Option<String>,
    pub descending: bool,
}
