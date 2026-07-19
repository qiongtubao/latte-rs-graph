//! Executor for the Cypher v1 subset.
//!
//! Takes a parsed `CypherQuery`, walks the storage layer, applies WHERE /
//! ORDER BY / LIMIT, and emits rows in the engine-facing
//! `Vec<CypherRow>` shape.
//!
//! The executor never touches SQL itself — the planner hands it pre-built
//! `SqlPlan`s. For variable-length paths the executor runs a Rust BFS
//! using the existing `edges_for_node` helper rather than relying on a
//! recursive CTE, which keeps worst-case memory bounded and avoids
//! huge SQL plans on dense graphs.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::GraphResult;
use crate::query::cypher::ast::{
    BinOp, CypherQuery, Expr, Literal, ReturnExpr, ReturnItem,
};
use crate::query::cypher::planner::{plan, PlanKind, SqlPlan};
use crate::storage::SqliteStorage;
use crate::types::{Edge, Node, NodeKind};

use rusqlite::{params_from_iter, ToSql};

// =============================================================================
// Row shape
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CypherRow {
    pub values: Vec<(String, CypherValue)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CypherValue {
    Node(Node),
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

// =============================================================================
// Entry point
// =============================================================================

/// Run a parsed Cypher query against the storage backend. Returns a flat
/// list of `CypherRow`s in the order the engine (and CLI) will render.
pub fn execute_cypher(
    query: &CypherQuery,
    storage: &SqliteStorage,
) -> GraphResult<Vec<CypherRow>> {
    let plan = plan(query).map_err(crate::error::GraphError::Config)?;
    let mut rows = match &plan.kind {
        PlanKind::SingleNode { var, .. } => exec_single_node(storage, &plan, var)?,
        PlanKind::DirectedEdge { from_var, to_var, edge_kind, from_label, to_label } => {
            exec_directed_edge(
                storage,
                &plan,
                from_var,
                to_var,
                edge_kind.as_deref(),
                from_label.as_deref(),
                to_label.as_deref(),
            )?
        }
        PlanKind::VariableLength {
            from_var,
            to_var,
            edge_kind,
            from_label,
            to_label,
            min_hops,
            max_hops,
        } => exec_variable_length(
            storage,
            &plan,
            from_var,
            to_var,
            edge_kind.as_deref(),
            from_label.as_deref(),
            to_label.as_deref(),
            *min_hops,
            *max_hops,
        )?,
    };

    // WHERE filter (over already-materialised bindings).
    if let Some(where_expr) = &query.where_clause {
        rows.retain(|row| row_matches(row, where_expr));
    }

    // ORDER BY (single column).
    if let Some(order) = &query.order_by {
        rows.sort_by(|a, b| {
            let av = lookup(a, &order.var, order.prop.as_deref())
                .unwrap_or(CypherValue::Null);
            let bv = lookup(b, &order.var, order.prop.as_deref())
                .unwrap_or(CypherValue::Null);
            let cmp = cmp_values(&av, &bv);
            if order.descending {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }

    // LIMIT.
    if let Some(limit) = query.limit {
        rows.truncate(limit as usize);
    }

    // Aggregate collapse: if the only RETURN item is COUNT(*), fold
    // every matched pattern row into a single aggregate row. Other
    // return expressions are left to project_rows.
    let rows = if is_count_star_only(&query.return_clause.items) {
        vec![CypherRow {
            values: vec![(
                query
                    .return_clause
                    .items
                    .first()
                    .and_then(|i| i.alias.clone())
                    .unwrap_or_else(|| "count(*)".to_string()),
                CypherValue::Int(rows.len() as i64),
            )],
        }]
    } else {
        rows
    };

    // Project via RETURN.
    let projected = project_rows(rows, &query.return_clause.items);
    Ok(projected)
}

fn is_count_star_only(items: &[crate::query::cypher::ast::ReturnItem]) -> bool {
    items.len() == 1
        && matches!(
            items[0].expr,
            crate::query::cypher::ast::ReturnExpr::CountStar
        )
}

// =============================================================================
// Pattern executors
// =============================================================================

fn exec_single_node(
    storage: &SqliteStorage,
    plan: &SqlPlan,
    var: &str,
) -> GraphResult<Vec<CypherRow>> {
    let nodes = storage.raw_query_nodes(&plan.sql, &plan.binds)?;
    Ok(nodes
        .into_iter()
        .map(|node| CypherRow {
            values: vec![(var.to_string(), CypherValue::Node(node))],
        })
        .collect())
}

fn exec_directed_edge(
    storage: &SqliteStorage,
    _plan: &SqlPlan,
    from_var: &str,
    to_var: &str,
    edge_kind: Option<&str>,
    from_label: Option<&str>,
    to_label: Option<&str>,
) -> GraphResult<Vec<CypherRow>> {
    // Pull the matching edges. We only fetch the edge columns we need,
    // then resolve source / target nodes via separate SELECTs. This is
    // cheaper than joining all 36 columns into one fat row and slicing
    // it back apart.
    let conn = storage.conn.lock().map_err(|e| {
        crate::error::GraphError::Engine(format!("sqlite lock poisoned: {e}"))
    })?;

    let mut sql = String::from(
        "SELECT id, source, target, kind, line, col, metadata, provenance \
         FROM edges WHERE valid = 1",
    );
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(k) = edge_kind {
        sql.push_str(" AND kind = ?");
        // Storage normalises edge kinds to lowercase (see
        // `EdgeKind::as_str`), so we must match that here.
        binds.push(Box::new(k.to_ascii_lowercase()));
    }
    let mut stmt = conn.prepare(&sql).map_err(crate::error::GraphError::Sqlite)?;
    let edge_iter = stmt
        .query_map(params_from_iter(binds.iter()), |row| {
            Ok(Edge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                kind: crate::types::EdgeKind::from_str(&row.get::<_, String>(3)?),
                line: row.get::<_, i64>(4)? as u32,
                col: row.get::<_, i64>(5)? as u32,
                metadata: row.get(6)?,
                provenance: row.get(7)?,
            })
        })
        .map_err(crate::error::GraphError::Sqlite)?;
    let edges: Vec<Edge> = edge_iter
        .collect::<Result<_, _>>()
        .map_err(crate::error::GraphError::Sqlite)?;
    drop(stmt);

    if edges.is_empty() {
        return Ok(Vec::new());
    }

    // Collect source / target ids separately, then look them up in one
    // batch each (so we issue at most two SELECTs).
    let mut src_ids: Vec<String> = edges.iter().map(|e| e.source.clone()).collect();
    let mut dst_ids: Vec<String> = edges.iter().map(|e| e.target.clone()).collect();
    src_ids.sort();
    src_ids.dedup();
    dst_ids.sort();
    dst_ids.dedup();

    let sources = nodes_by_ids(&conn, &src_ids, from_label)?;
    let targets = nodes_by_ids(&conn, &dst_ids, to_label)?;

    let mut out = Vec::with_capacity(edges.len());
    for edge in &edges {
        let Some(from_node) = sources.get(&edge.source) else { continue; };
        let Some(to_node) = targets.get(&edge.target) else { continue; };
        out.push(CypherRow {
            values: vec![
                (from_var.to_string(), CypherValue::Node(from_node.clone())),
                (to_var.to_string(), CypherValue::Node(to_node.clone())),
            ],
        });
    }

    Ok(out)
}

fn exec_variable_length(
    storage: &SqliteStorage,
    plan: &SqlPlan,
    from_var: &str,
    to_var: &str,
    edge_kind: Option<&str>,
    from_label: Option<&str>,
    to_label: Option<&str>,
    min_hops: u32,
    max_hops: u32,
) -> GraphResult<Vec<CypherRow>> {
    let seeds = storage.raw_query_nodes(&plan.sql, &plan.binds)?;
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let target_kind = to_label.map(|l| NodeKind::from_str(&l.to_ascii_lowercase()));

    let mut out: Vec<CypherRow> = Vec::new();
    for seed in &seeds {
        // BFS forward along edges_for_node, capped at max_hops.
        let mut frontier: Vec<(String, u32)> = vec![(seed.id.clone(), 0)];
        let mut visited: HashSet<(String, u32)> = HashSet::new();
        visited.insert((seed.id.clone(), 0));

        while let Some((current_id, depth)) = frontier.pop() {
            if depth >= max_hops {
                continue;
            }
            let edges = storage.edges_for_node(&current_id)?;
            for edge in &edges {
                if edge.kind == crate::types::EdgeKind::Other(String::new()) {
                    continue;
                }
                if let Some(ek) = edge_kind {
                    if edge.kind.as_str() != ek.to_ascii_lowercase() {
                        continue;
                    }
                }
                if edge.source != current_id {
                    // We only walk forward along outgoing edges in v1.
                    continue;
                }
                let next_id = edge.target.clone();
                let next_depth = depth + 1;
                if !visited.insert((next_id.clone(), next_depth)) {
                    continue;
                }
                frontier.push((next_id.clone(), next_depth));
            }
        }

        // For every visited node at depth >= min_hops, fetch the
        // target node and (optionally) filter by to-label.
        let visited_ids: Vec<String> = visited
            .iter()
            .filter(|(_, d)| *d >= min_hops)
            .map(|(id, _)| id.clone())
            .collect();
        if visited_ids.is_empty() {
            continue;
        }
        // Deduplicate IDs (a node reachable at multiple depths should
        // produce only one row pair per `seed`).
        let mut unique: Vec<String> = visited_ids.clone();
        unique.sort();
        unique.dedup();

        let conn = storage.conn.lock().map_err(|e| {
            crate::error::GraphError::Engine(format!("sqlite lock poisoned: {e}"))
        })?;
        let targets = nodes_by_ids(&conn, &unique, to_label)?;
        drop(conn);

        for id in &unique {
            let Some(node) = targets.get(id) else { continue; };
            if let Some(kind) = &target_kind {
                if node.kind != *kind {
                    continue;
                }
            }
            // From-label is already enforced by the seed query; nothing
            // else to filter on the from side here.
            let _ = from_label;
            out.push(CypherRow {
                values: vec![
                    (from_var.to_string(), CypherValue::Node(seed.clone())),
                    (to_var.to_string(), CypherValue::Node(node.clone())),
                ],
            });
        }
    }

    Ok(out)
}

fn nodes_by_ids(
    conn: &rusqlite::Connection,
    ids: &[String],
    label: Option<&str>,
) -> GraphResult<HashMap<String, Node>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // Build "(?, ?, …)" with N placeholders.
    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, kind, name, qualified_name, file_path, language, \
                start_line, end_line, start_column, end_column, \
                signature, docstring, visibility, \
                is_exported, is_async, is_static, is_abstract, extra \
         FROM nodes WHERE valid = 1 AND id IN ({placeholders})"
    );
    let mut binds: Vec<Box<dyn ToSql>> = Vec::with_capacity(ids.len());
    for id in ids {
        binds.push(Box::new(id.clone()));
    }
    // `label` is filtered in Rust below — we don't bind it as a SQL
    // parameter because the SQL only has the `id IN (...)` placeholders.

    let mut stmt = conn.prepare(&sql).map_err(crate::error::GraphError::Sqlite)?;
    let row_iter = stmt
        .query_map(params_from_iter(binds.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)? as u32,
                row.get::<_, i64>(7)? as u32,
                row.get::<_, i64>(8)? as u32,
                row.get::<_, i64>(9)? as u32,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, i64>(13)? != 0,
                row.get::<_, i64>(14)? != 0,
                row.get::<_, i64>(15)? != 0,
                row.get::<_, i64>(16)? != 0,
                row.get::<_, String>(17)?,
            ))
        })
        .map_err(crate::error::GraphError::Sqlite)?;

    let mut out = HashMap::new();
    for row in row_iter {
        let (id, kind, name, qname, file_path, language, sl, el, sc, ec, sig, doc, vis,
             is_exp, is_async, is_static, is_abstract, extra_str) =
            row.map_err(crate::error::GraphError::Sqlite)?;
        if let Some(l) = label {
            if kind.to_ascii_lowercase() != l.to_ascii_lowercase() {
                continue;
            }
        }
        let extra: std::collections::HashMap<String, String> =
            serde_json::from_str(&extra_str).unwrap_or_default();
        let node = Node {
            id,
            kind: NodeKind::from_str(&kind),
            name,
            qualified_name: qname,
            file_path,
            language,
            start_line: sl,
            end_line: el,
            start_column: sc,
            end_column: ec,
            signature: sig,
            docstring: doc,
            visibility: vis,
            is_exported: is_exp,
            is_async,
            is_static,
            is_abstract,
            extra,
        };
        out.insert(node.id.clone(), node);
    }

    Ok(out)
}

// =============================================================================
// Filtering / projection helpers
// =============================================================================

fn row_matches(row: &CypherRow, expr: &Expr) -> bool {
    match expr {
        Expr::Property { var, prop } => {
            if let Some(v) = lookup(row, var, Some(prop)) {
                !matches!(v, CypherValue::Null)
            } else {
                false
            }
        }
        Expr::BinaryOp { op, left, right } => {
            let l = eval_expr(row, left);
            let r = eval_expr(row, right);
            match (op, l, r) {
                (BinOp::Eq, CypherValue::Null, _) | (BinOp::Eq, _, CypherValue::Null) => false,
                (BinOp::Ne, CypherValue::Null, _) | (BinOp::Ne, _, CypherValue::Null) => false,
                (BinOp::Eq, a, b) => cmp_values(&a, &b).is_eq(),
                (BinOp::Ne, a, b) => !cmp_values(&a, &b).is_eq(),
                (BinOp::Lt, a, b) => cmp_values(&a, &b).is_lt(),
                (BinOp::Le, a, b) => {
                    let c = cmp_values(&a, &b);
                    c.is_lt() || c.is_eq()
                }
                (BinOp::Gt, a, b) => cmp_values(&a, &b).is_gt(),
                (BinOp::Ge, a, b) => {
                    let c = cmp_values(&a, &b);
                    c.is_gt() || c.is_eq()
                }
            }
        }
        Expr::InList { var, prop, values } => {
            let Some(target) = lookup(row, var, Some(prop)) else { return false; };
            values.iter().any(|v| literal_eq_value(v, &target))
        }
        Expr::StartsWith { var, prop, value } => {
            let Some(target) = lookup(row, var, Some(prop)) else { return false; };
            match (target, value) {
                (CypherValue::Str(s), Literal::Str(prefix)) => s.starts_with(prefix.as_str()),
                _ => false,
            }
        }
        Expr::Contains { var, prop, value } => {
            let Some(target) = lookup(row, var, Some(prop)) else { return false; };
            match (target, value) {
                (CypherValue::Str(s), Literal::Str(needle)) => s.contains(needle.as_str()),
                _ => false,
            }
        }
        Expr::Var(_) | Expr::Literal(_) => true,
    }
}

fn eval_expr(row: &CypherRow, expr: &Expr) -> CypherValue {
    match expr {
        Expr::Var(name) => lookup(row, name, None).unwrap_or(CypherValue::Null),
        Expr::Property { var, prop } => {
            lookup(row, var, Some(prop)).unwrap_or(CypherValue::Null)
        }
        Expr::Literal(l) => value_from_literal(l),
        _ => CypherValue::Null,
    }
}

fn value_from_literal(l: &Literal) -> CypherValue {
    match l {
        Literal::Str(s) => CypherValue::Str(s.clone()),
        Literal::Int(n) => CypherValue::Int(*n),
        Literal::Float(n) => CypherValue::Float(*n),
        Literal::Bool(b) => CypherValue::Bool(*b),
        Literal::Null => CypherValue::Null,
    }
}

fn literal_eq_value(lit: &Literal, val: &CypherValue) -> bool {
    match (lit, val) {
        (Literal::Str(a), CypherValue::Str(b)) => a == b,
        (Literal::Int(a), CypherValue::Int(b)) => a == b,
        (Literal::Float(a), CypherValue::Float(b)) => a == b,
        (Literal::Float(a), CypherValue::Int(b)) => *a == *b as f64,
        (Literal::Int(a), CypherValue::Float(b)) => *a as f64 == *b,
        (Literal::Bool(a), CypherValue::Bool(b)) => a == b,
        (Literal::Null, CypherValue::Null) => true,
        _ => false,
    }
}

fn lookup(row: &CypherRow, var: &str, prop: Option<&str>) -> Option<CypherValue> {
    for (name, value) in &row.values {
        if name == var {
            return match (prop, value) {
                (None, _) => Some(value.clone()),
                (Some(p), CypherValue::Node(n)) => node_property(n, p),
                _ => Some(value.clone()),
            };
        }
    }
    None
}

fn node_property(node: &Node, prop: &str) -> Option<CypherValue> {
    match prop {
        "id" => Some(CypherValue::Str(node.id.clone())),
        "name" => Some(CypherValue::Str(node.name.clone())),
        "qualified_name" => Some(CypherValue::Str(node.qualified_name.clone())),
        "file_path" => Some(CypherValue::Str(node.file_path.clone())),
        "kind" => Some(CypherValue::Str(node.kind.as_str().to_string())),
        "language" => Some(CypherValue::Str(node.language.clone())),
        "is_exported" => Some(CypherValue::Bool(node.is_exported)),
        "is_async" => Some(CypherValue::Bool(node.is_async)),
        "start_line" => Some(CypherValue::Int(node.start_line as i64)),
        "end_line" => Some(CypherValue::Int(node.end_line as i64)),
        // Anything else: try the node.extra map, fall back to None.
        _ => node.extra.get(prop).map(|v| CypherValue::Str(v.clone())),
    }
}

fn cmp_values(a: &CypherValue, b: &CypherValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (CypherValue::Int(x), CypherValue::Int(y)) => x.cmp(y),
        (CypherValue::Float(x), CypherValue::Float(y)) => {
            x.partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (CypherValue::Int(x), CypherValue::Float(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (CypherValue::Float(x), CypherValue::Int(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (CypherValue::Str(x), CypherValue::Str(y)) => x.cmp(y),
        (CypherValue::Bool(x), CypherValue::Bool(y)) => x.cmp(y),
        (CypherValue::Null, CypherValue::Null) => Ordering::Equal,
        _ => Ordering::Equal,
    }
}

// =============================================================================
// Projection
// =============================================================================

fn project_rows(rows: Vec<CypherRow>, items: &[ReturnItem]) -> Vec<CypherRow> {
    // If the only item is COUNT(*), the rows have already been
    // collapsed upstream into a single aggregate row — pass them
    // through verbatim instead of clobbering the precomputed value
    // with the per-row `Int(1)` fallback in `project_one`.
    if items.len() == 1
        && matches!(items[0].expr, ReturnExpr::CountStar)
    {
        return rows;
    }
    rows.into_iter()
        .map(|row| {
            let values = items
                .iter()
                .map(|item| project_one(&row, item))
                .collect();
            CypherRow { values }
        })
        .collect()
}

fn project_one(row: &CypherRow, item: &ReturnItem) -> (String, CypherValue) {
    let key = item
        .alias
        .clone()
        .unwrap_or_else(|| default_return_key(&item.expr));
    let value = match &item.expr {
        ReturnExpr::Var(name) => {
            lookup(row, name, None).unwrap_or(CypherValue::Null)
        }
        ReturnExpr::Property { var, prop } => {
            lookup(row, var, Some(prop)).unwrap_or(CypherValue::Null)
        }
        ReturnExpr::CountVar(name) => {
            // Count the number of bound occurrences of `name` in this row.
            // If the variable isn't bound, count 0.
            let n = row
                .values
                .iter()
                .filter(|(n, _)| n == name)
                .count() as i64;
            CypherValue::Int(n.max(1)) // unbound → at least 1 so total >= 1.
        }
        ReturnExpr::CountStar => {
            // COUNT(*) over a row always yields 1.
            CypherValue::Int(1)
        }
    };
    (key, value)
}

fn default_return_key(expr: &ReturnExpr) -> String {
    match expr {
        ReturnExpr::Var(s) => s.clone(),
        ReturnExpr::Property { var, prop } => format!("{var}.{prop}"),
        ReturnExpr::CountVar(s) => format!("count({s})"),
        ReturnExpr::CountStar => "count(*)".to_string(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::cypher::parser::parse_cypher;
    use crate::storage::SqliteStorage;
    use crate::types::{Edge, EdgeKind, FileRecord, Node, NodeKind};
    use tempfile::TempDir;

    /// Build an in-memory graph: `caller -> sum_pairs`, `caller -> other`,
    /// `other -> sum_pairs`, plus a stray `unrelated` function.
    fn bootstrap() -> (TempDir, SqliteStorage) {
        let dir = TempDir::new().expect("tempdir");
        let storage = SqliteStorage::open(&dir.path().join("graph.db")).expect("open db");

        let mk = |id: &str, name: &str, file: &str, line: u32| Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: name.to_string(),
            qualified_name: format!("{file}::{name}"),
            file_path: file.to_string(),
            language: "rust".to_string(),
            start_line: line,
            end_line: line,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: Some("pub".to_string()),
            is_exported: true,
            is_async: false,
            is_static: false,
            is_abstract: false,
            extra: HashMap::new(),
        };

        let caller = mk("function:caller", "caller", "src/a.rs", 1);
        let sum_pairs = mk("function:sum_pairs", "sum_pairs", "src/a.rs", 5);
        let other = mk("function:other", "other", "src/b.rs", 1);
        let unrelated = mk("function:unrelated", "unrelated", "src/c.rs", 1);

        storage.upsert_file(&FileRecord {
            path: "src/a.rs".to_string(),
            language: "rust".to_string(),
            mtime: 0,
            content_hash: String::new(),
            indexed_at: 0,
        }).unwrap();
        storage.upsert_file(&FileRecord {
            path: "src/b.rs".to_string(),
            language: "rust".to_string(),
            mtime: 0,
            content_hash: String::new(),
            indexed_at: 0,
        }).unwrap();
        storage.upsert_file(&FileRecord {
            path: "src/c.rs".to_string(),
            language: "rust".to_string(),
            mtime: 0,
            content_hash: String::new(),
            indexed_at: 0,
        }).unwrap();

        for n in [&caller, &sum_pairs, &other, &unrelated] {
            storage.upsert_node(n).unwrap();
        }

        let mk_edge = |id: &str, src: &str, dst: &str, kind: EdgeKind| Edge {
            id: id.to_string(),
            source: src.to_string(),
            target: dst.to_string(),
            kind,
            line: 1,
            col: 0,
            metadata: None,
            provenance: None,
        };

        storage
            .upsert_edge(&mk_edge("e1", &caller.id, &sum_pairs.id, EdgeKind::Calls))
            .unwrap();
        storage
            .upsert_edge(&mk_edge("e2", &caller.id, &other.id, EdgeKind::Calls))
            .unwrap();
        storage
            .upsert_edge(&mk_edge("e3", &other.id, &sum_pairs.id, EdgeKind::Calls))
            .unwrap();

        (dir, storage)
    }

    fn names(rows: &[CypherRow], key: &str) -> Vec<String> {
        let mut out: Vec<String> = rows
            .iter()
            .filter_map(|r| {
                r.values.iter().find_map(|(k, v)| {
                    if k == key {
                        match v {
                            CypherValue::Str(s) => Some(s.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn exec_single_node_match() {
        let (_dir, storage) = bootstrap();
        let q = parse_cypher("MATCH (n:Function) RETURN n.name AS name")
            .unwrap();
        let rows = execute_cypher(&q, &storage).unwrap();
        let got = names(&rows, "name");
        assert_eq!(
            got,
            vec![
                "caller".to_string(),
                "other".to_string(),
                "sum_pairs".to_string(),
                "unrelated".to_string(),
            ]
        );
    }

    #[test]
    fn exec_relationship_callers() {
        let (_dir, storage) = bootstrap();
        let q = parse_cypher(
            "MATCH (n:Function)-[:CALLS]->(m:Function) RETURN m.name AS callee",
        )
        .unwrap();
        let rows = execute_cypher(&q, &storage).unwrap();
        let got = names(&rows, "callee");
        // Edges: caller→sum_pairs, caller→other, other→sum_pairs
        assert_eq!(
            got,
            vec![
                "other".to_string(),
                "sum_pairs".to_string(),
                "sum_pairs".to_string(),
            ]
        );
    }

    #[test]
    fn exec_where_starts_with() {
        let (_dir, storage) = bootstrap();
        let q = parse_cypher(
            "MATCH (n:Function) WHERE n.name STARTS WITH 'sum' RETURN n.name AS name",
        )
        .unwrap();
        let rows = execute_cypher(&q, &storage).unwrap();
        assert_eq!(names(&rows, "name"), vec!["sum_pairs".to_string()]);
    }

    #[test]
    fn exec_variable_length_walks() {
        let (_dir, storage) = bootstrap();
        // caller→sum_pairs (depth 1), caller→other→sum_pairs (depth 2)
        let q = parse_cypher(
            "MATCH (a:Function)-[:CALLS*1..2]->(b:Function) RETURN DISTINCT b.name AS name",
        )
        .unwrap();
        let rows = execute_cypher(&q, &storage).unwrap();
        let got = names(&rows, "name");
        assert!(got.contains(&"sum_pairs".to_string()));
        assert!(got.contains(&"other".to_string()));
    }

    #[test]
    fn exec_count_star() {
        let (_dir, storage) = bootstrap();
        let q = parse_cypher("MATCH (n:Function) RETURN COUNT(*) AS total")
            .unwrap();
        let rows = execute_cypher(&q, &storage).unwrap();
        // COUNT(*) is the only RETURN item; the executor collapses the
        // single-row aggregate case to one row carrying the count.
        // v1 pragmatic semantics: row count == number of matched
        // pattern rows (i.e. the function nodes), not deduplicated.
        assert!(!rows.is_empty());
        let total = rows
            .iter()
            .flat_map(|r| r.values.iter())
            .find_map(|(_, v)| match v {
                CypherValue::Int(n) => Some(*n),
                _ => None,
            })
            .expect("total present");
        let function_count = storage
            .all_nodes()
            .unwrap()
            .iter()
            .filter(|n| n.kind == NodeKind::Function)
            .count() as i64;
        assert_eq!(total, function_count);
    }
}
