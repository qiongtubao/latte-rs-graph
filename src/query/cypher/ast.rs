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
    /// One main MATCH clause (v1: exactly 1). Kept as a Vec so the
    /// grammar can grow toward multi-MATCH later without another AST
    /// rewrite.
    pub match_clauses: Vec<MatchClause>,
    /// Zero or more left-outer-joined OPTIONAL MATCH clauses. v1
    /// accepts multiple in the parser but the executor only handles 1
    /// — anything beyond that is rejected at execution time.
    pub optional_clauses: Vec<MatchClause>,
    pub return_clause: ReturnClause,
    pub order_by: Option<OrderBy>,
    pub limit: Option<u32>,
    /// Right-hand side of a UNION [ALL] chain. v1 allows at most one
    /// hop (the right side itself is a fresh `CypherQuery` without
    /// nested UNIONs — the parser enforces that).
    pub union_next: Option<Box<CypherQuery>>,
    /// `true` for `UNION ALL`, `false` for the default distinct UNION.
    pub union_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchClause {
    pub pattern: Pattern,
    /// `WHERE` that scopes this specific MATCH / OPTIONAL MATCH. A
    /// `WHERE` appearing after the main MATCH applies to it; a `WHERE`
    /// appearing after an `OPTIONAL MATCH` applies to that optional
    /// clause. The parser binds each `WHERE` to the immediately
    /// preceding MATCH / OPTIONAL MATCH token.
    pub where_clause: Option<Expr>,
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
