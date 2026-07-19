//! Translate the parsed Cypher AST into the inputs the executor needs.
//!
//! v1 is intentionally simple:
//! - Single-node patterns: hand the executor a SQL string + binds that
//!   filters by label (if any) and returns node rows.
//! - Directed-edge patterns: SQL JOIN across edges + two copies of nodes,
//!   filtered by source / target labels (if any).
//! - Variable-length patterns: planner emits the seed-fetch SQL plus the
//!   BFS caps; the executor walks the edges itself to avoid long-running
//!   recursive CTEs on huge graphs.
//!
//! WHERE / ORDER BY / LIMIT / RETURN projection are handled by the
//! executor over the rows the planner produced — they don't translate to
//! SQL here. That's deliberate: keeps the planner small and the executor
//! in charge of presentation.

use crate::query::cypher::ast::{
    CypherQuery, EdgePattern, NodePattern, Pattern,
};

#[derive(Debug, Clone)]
pub struct SqlPlan {
    pub kind: PlanKind,
    /// Select node row(s). SELECT clause lists all node columns; the
    /// executor materialises them via `row_to_node`.
    pub sql: String,
    /// Bind parameters for `?` placeholders in `sql` (left-to-right).
    pub binds: Vec<crate::query::cypher::ast::Literal>,
    /// Variables that surface as Node bindings in each row, in column
    /// order (used by the executor to slice rows from `sql` output).
    pub node_vars: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum PlanKind {
    SingleNode {
        var: String,
        label: Option<String>,
    },
    DirectedEdge {
        from_var: String,
        to_var: String,
        edge_kind: Option<String>,
        from_label: Option<String>,
        to_label: Option<String>,
    },
    VariableLength {
        from_var: String,
        to_var: String,
        edge_kind: Option<String>,
        from_label: Option<String>,
        to_label: Option<String>,
        min_hops: u32,
        max_hops: u32,
    },
}

const NODE_SELECT: &str = "id, kind, name, qualified_name, file_path, language, \
     start_line, end_line, start_column, end_column, \
     signature, docstring, visibility, \
     is_exported, is_async, is_static, is_abstract, extra";

/// Translate the parsed query into a SQL-backed plan. Most filtering is
/// pushed down to the WHERE clauses; the executor fills in any remaining
/// `WHERE` / `RETURN` / `ORDER BY` / `LIMIT` work.
pub fn plan(query: &CypherQuery) -> Result<SqlPlan, String> {
    // Used for WHERE on a property — the planner lifts simple equality
    // filters into the SQL bind list when it can. For v1 we just stash
    // every WHERE into `plan` as a no-op SQL hint; the executor still
    // applies it. This keeps things simple while leaving the door open
    // for richer pushdown later.

    match &query.match_clauses[0].pattern {
        Pattern::SingleNode(np) => Ok(SqlPlan {
            kind: PlanKind::SingleNode {
                var: np.var.clone(),
                label: np.label.clone(),
            },
            sql: build_single_node_sql(np)?,
            binds: bind_for_label(np.label.as_deref()),
            node_vars: vec![np.var.clone()],
        }),
        Pattern::DirectedEdge { from, edge, to } => {
            let ek = edge_kind_filter(edge)?;
            Ok(SqlPlan {
                kind: PlanKind::DirectedEdge {
                    from_var: from.var.clone(),
                    to_var: to.var.clone(),
                    edge_kind: ek,
                    from_label: from.label.clone(),
                    to_label: to.label.clone(),
                },
                sql: build_directed_edge_sql(from, to)?,
                binds: bind_for_label(from.label.as_deref()),
                node_vars: vec![from.var.clone(), to.var.clone()],
            })
        }
        Pattern::VariableLength {
            from,
            edge,
            min_hops,
            max_hops,
            to,
        } => {
            let ek = edge_kind_filter(edge)?;
            Ok(SqlPlan {
                kind: PlanKind::VariableLength {
                    from_var: from.var.clone(),
                    to_var: to.var.clone(),
                    edge_kind: ek,
                    from_label: from.label.clone(),
                    to_label: to.label.clone(),
                    min_hops: *min_hops,
                    max_hops: *max_hops,
                },
                sql: build_single_node_sql(from)?,
                binds: bind_for_label(from.label.as_deref()),
                node_vars: vec![from.var.clone(), to.var.clone()],
            })
        }
    }
}

fn bind_for_label(label: Option<&str>) -> Vec<crate::query::cypher::ast::Literal> {
    label
        .map(|l| vec![crate::query::cypher::ast::Literal::Str(l.to_ascii_lowercase())])
        .unwrap_or_default()
}

fn build_single_node_sql(np: &NodePattern) -> Result<String, String> {
    // Two-column layout: the executor reads column 0..18 as the Node.
    // Column 18 is reserved (1 for from, 1 for to) — variable-length
    // reuses the same layout via UNION ALL with a leading node SELECT.
    let label_clause = if np.label.is_some() {
        " AND n.kind = ?".to_string()
    } else {
        String::new()
    };
    Ok(format!(
        "SELECT {NODE_SELECT} FROM nodes n WHERE n.valid = 1{label_clause}"
    ))
}

fn build_directed_edge_sql(
    from: &NodePattern,
    to: &NodePattern,
) -> Result<String, String> {
    let label_from = if from.label.is_some() {
        " AND a.kind = ?".to_string()
    } else {
        String::new()
    };
    let label_to = if to.label.is_some() {
        " AND b.kind = ?".to_string()
    } else {
        String::new()
    };
    Ok(format!(
        "SELECT {a_cols}, {b_cols} \
         FROM edges e \
         JOIN nodes a ON a.id = e.source AND a.valid = 1{label_from} \
         JOIN nodes b ON b.id = e.target AND b.valid = 1{label_to} \
         WHERE e.valid = 1",
        a_cols = NODE_SELECT.replace("n.", "a."),
        b_cols = NODE_SELECT.replace("n.", "b."),
    ))
}

fn edge_kind_filter(edge: &EdgePattern) -> Result<Option<String>, String> {
    Ok(edge.kind.clone())
}

// =============================================================================
// Tiny self-test: round-trip a known query so this module stays
// exercised. Heavier tests live in the parser/executor modules.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::cypher::parser::parse_cypher;

    fn round_trip(sql: &str) -> SqlPlan {
        let q = parse_cypher(sql).expect("parse");
        plan(&q).expect("plan")
    }

    #[test]
    fn single_node_emits_select_with_label_bind() {
        let p = round_trip("MATCH (n:Function) RETURN n");
        match &p.kind {
            PlanKind::SingleNode { var, label } if var == "n" && label.as_deref() == Some("Function") => {}
            other => panic!("expected SingleNode n/Function, got {other:?}"),
        }
        assert!(p.sql.contains("FROM nodes n"));
        assert_eq!(p.binds.len(), 1);
    }
    #[test]
    fn directed_edge_emits_two_node_join() {
        let p = round_trip("MATCH (n:Function)-[:CALLS]->(m) RETURN n, m");
        match &p.kind {
            PlanKind::DirectedEdge { edge_kind, .. } => {
                assert_eq!(edge_kind.as_deref(), Some("CALLS"));
            }
            other => panic!("expected DirectedEdge, got {other:?}"),
        }
        assert!(p.sql.contains("JOIN nodes a"));
        assert!(p.sql.contains("JOIN nodes b"));
    }

    #[test]
    fn variable_length_carries_hops() {
        let p = round_trip("MATCH (a:Function)-[:CALLS*1..3]->(b) RETURN b");
        match &p.kind {
            PlanKind::VariableLength { min_hops, max_hops, .. } => {
                assert_eq!(*min_hops, 1);
                assert_eq!(*max_hops, 3);
            }
            other => panic!("expected VariableLength, got {other:?}"),
        }
    }
}
