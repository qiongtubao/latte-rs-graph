//! Complexity metrics computed at parse time and stored on `Node.extra`.
//!
//! For each indexed function / method, we walk its AST subtree once and tally
//! branch / loop / recursion / scan / alloc signals. The result is stuffed
//! into `node.extra` so it round-trips through storage without schema
//! changes, and can be rebuilt with [`metrics_from_extra`] at read time.

use std::collections::HashMap;
use tree_sitter::Node;

use crate::types::ComplexityMetrics;

// =============================================================================
// Public API
// =============================================================================

/// Compute complexity metrics for a function/method body.
///
/// * `func_root` is the tree-sitter node whose subtree is the function body
///   (typically a `function_item` / `function_definition` / `method_definition`).
/// * `source` is the full source bytes (UTF-8).
/// * `func_name` is the function's simple name (last segment of qualified_name).
/// * `lang` is the language name (e.g. `"rust"`, `"python"`, `"typescript"`).
///   Unknown values fall back to Rust-flavoured kinds.
pub fn compute_metrics(
    source: &[u8],
    func_root: Node,
    func_name: &str,
    lang: &str,
) -> ComplexityMetrics {
    let kinds = lang_kinds(lang);
    let mut state = WalkState::default();
    walk(source, func_root, &kinds, &mut state, 0);

    // Recursion is detected in a separate pass so the primary walk can stay
    // tight and recursion-related bookkeeping never bleeds into cyclomatic /
    // cognitive / depth accounting.
    detect_recursion(source, func_root, func_name, &kinds, &mut state);

    ComplexityMetrics {
        cyclomatic: 1 + state.branch_count,
        cognitive: state.cognitive,
        max_loop_depth: state.max_loop_depth,
        alloc_in_loop: state.alloc_in_loop,
        linear_scan_in_loop: state.linear_scan_in_loop,
        is_recursive: state.is_recursive,
        unguarded_recursion: state.unguarded_recursion,
    }
}

/// Serialize a `ComplexityMetrics` into the 7 keys consumed by [`metrics_from_extra`].
pub fn metrics_to_extra(m: &ComplexityMetrics) -> HashMap<String, String> {
    let mut h = HashMap::with_capacity(7);
    h.insert("cyclomatic".to_string(), m.cyclomatic.to_string());
    h.insert("cognitive".to_string(), m.cognitive.to_string());
    h.insert("max_loop_depth".to_string(), m.max_loop_depth.to_string());
    h.insert("alloc_in_loop".to_string(), m.alloc_in_loop.to_string());
    h.insert("linear_scan_in_loop".to_string(), m.linear_scan_in_loop.to_string());
    h.insert("recursion".to_string(), m.is_recursive.to_string());
    h.insert("unguarded_recursion".to_string(), m.unguarded_recursion.to_string());
    h
}

/// Inverse of [`metrics_to_extra`]. Returns `None` if any required key is
/// missing or unparsable.
pub fn metrics_from_extra(extra: &HashMap<String, String>) -> Option<ComplexityMetrics> {
    Some(ComplexityMetrics {
        cyclomatic: extra.get("cyclomatic")?.parse().ok()?,
        cognitive: extra.get("cognitive")?.parse().ok()?,
        max_loop_depth: extra.get("max_loop_depth")?.parse().ok()?,
        alloc_in_loop: extra.get("alloc_in_loop")?.parse().ok()?,
        linear_scan_in_loop: extra.get("linear_scan_in_loop")?.parse().ok()?,
        is_recursive: extra.get("recursion")?.parse().ok()?,
        unguarded_recursion: extra.get("unguarded_recursion")?.parse().ok()?,
    })
}

// =============================================================================
// Walking state
// =============================================================================

#[derive(Default)]
struct WalkState {
    /// Count of branches / loops / logical-operator sequences encountered.
    /// `cyclomatic` = 1 + this.
    branch_count: u32,
    /// SonarSource-style cognitive complexity (see module docs).
    cognitive: u32,
    max_loop_depth: u32,
    alloc_in_loop: bool,
    linear_scan_in_loop: bool,
    in_loop: u32,

    // Recursion bookkeeping (populated by `detect_recursion`).
    is_recursive: bool,
    unguarded_recursion: bool,
}

// =============================================================================
// Per-language kind tables
// =============================================================================

struct LangKinds {
    /// Node kinds that count as a branch point (contribute to cyclomatic and
    /// to cognitive) but are NOT themselves loops.
    branch: &'static [&'static str],
    /// Node kinds that are loops: contribute to cyclomatic, cognitive, and
    /// `max_loop_depth`.
    loop_kinds: &'static [&'static str],
    /// Node kind for a function/method call (used by recursion detection).
    call_kind: &'static str,
    /// Node kinds treated as `if` for the recursion-guard check.
    if_kind: &'static [&'static str],
    /// Node kinds that act as a `return` inside an if-body for the guard check.
    return_kinds: &'static [&'static str],
    /// Node kind that wraps a logical operator (`&&` / `||`); we then look at
    /// the named `operator` field for the operator text.
    bool_op_kind: Option<&'static str>,
    /// Operators that count as a short-circuit branch.
    bool_ops: &'static [&'static str],
    /// Node kinds we pattern-match for allocation markers (`vec!`,
    /// `Vec::new`, `Box::new`, `.collect(`, etc.) while inside a loop.
    alloc_match_kinds: &'static [&'static str],
    /// Substrings that signal an allocation when found in the text of any
    /// `alloc_match_kinds` node.
    alloc_substrings: &'static [&'static str],
    /// Node kinds we pattern-match for linear-scan markers (`.find(`,
    /// `.contains(`, etc.) while inside a loop.
    scan_match_kinds: &'static [&'static str],
    /// Substrings that signal a linear scan when found in the text of any
    /// `scan_match_kinds` node.
    scan_substrings: &'static [&'static str],
}

fn lang_kinds(lang: &str) -> LangKinds {
    match lang {
        "rust" => LangKinds {
            branch: &[
                "if_expression",
                "match_arm",
                "if_let_expression",
                "try_expression",
            ],
            loop_kinds: &["for_expression", "while_expression", "loop_expression"],
            call_kind: "call_expression",
            if_kind: &["if_expression"],
            return_kinds: &["return_expression"],
            bool_op_kind: Some("binary_expression"),
            bool_ops: &["&&", "||"],
            // Macro invocations (vec!) and call expressions (Vec::new, .collect)
            // both show up at the top of an expression.
            alloc_match_kinds: &["macro_invocation", "call_expression", "array_expression"],
            alloc_substrings: &[
                "vec!",
                "Vec::new",
                "Box::new",
                "String::from",
                ".collect(",
                "HashMap::new",
                "BTreeMap::new",
                "Vec::with_capacity",
            ],
            scan_match_kinds: &["call_expression"],
            scan_substrings: &[".find(", ".contains(", ".indexOf(", ".binary_search(", ".includes("],
        },
        "python" => LangKinds {
            branch: &[
                "if_statement",
                "elif_clause",
                "case_clause",
                "try_statement",
            ],
            loop_kinds: &["for_statement", "while_statement"],
            call_kind: "call",
            if_kind: &["if_statement"],
            return_kinds: &["return_statement"],
            bool_op_kind: Some("boolean_operator"),
            bool_ops: &["and", "or"],
            alloc_match_kinds: &["call", "list"],
            alloc_substrings: &["list(", "dict(", "set(", "[", ".append("],
            scan_match_kinds: &["call"],
            scan_substrings: &[".find(", ".contains(", ".index(", " in "],
        },
        "typescript" | "javascript" => LangKinds {
            branch: &[
                "if_statement",
                "switch_case",
                "try_statement",
                "ternary_expression",
            ],
            loop_kinds: &[
                "for_statement",
                "for_in_statement",
                "while_statement",
                "do_statement",
            ],
            call_kind: "call_expression",
            if_kind: &["if_statement"],
            return_kinds: &["return_statement"],
            bool_op_kind: Some("binary_expression"),
            bool_ops: &["&&", "||", "??"],
            alloc_match_kinds: &["call_expression", "array"],
            alloc_substrings: &[
                "new ",
                ".map(",
                ".filter(",
                ".fromEntries(",
                ".concat(",
                ".push(",
                ".slice(",
            ],
            scan_match_kinds: &["call_expression"],
            scan_substrings: &[".find(", ".findIndex(", ".includes(", ".indexOf(", ".some(", ".every("],
        },
        "c" | "cpp" => LangKinds {
            branch: &["if_statement", "case_statement", "switch_case"],
            loop_kinds: &["for_statement", "while_statement", "do_statement"],
            call_kind: "call_expression",
            if_kind: &["if_statement"],
            return_kinds: &["return_statement"],
            bool_op_kind: Some("binary_expression"),
            bool_ops: &["&&", "||"],
            alloc_match_kinds: &["call_expression", "array_initializer"],
            alloc_substrings: &["malloc(", "calloc(", "realloc(", "new "],
            scan_match_kinds: &["call_expression"],
            scan_substrings: &["strstr(", "memchr(", ".find(", ".contains("],
        },
        "go" => LangKinds {
            branch: &[
                "if_statement",
                "switch_case",
                "select_statement",
                "type_switch_case",
            ],
            loop_kinds: &["for_statement"],
            call_kind: "call_expression",
            if_kind: &["if_statement"],
            return_kinds: &["return_statement"],
            bool_op_kind: Some("binary_expression"),
            bool_ops: &["&&", "||"],
            alloc_match_kinds: &["call_expression", "composite_literal"],
            alloc_substrings: &["make(", "new(", "append("],
            scan_match_kinds: &["call_expression"],
            scan_substrings: &[".find(", ".contains(", "strings.Contains"],
        },
        // Unknown languages fall back to the Rust table — gives best-effort
        // metrics for any C-family dialect.
        _ => lang_kinds("rust"),
    }
}

// =============================================================================
// Primary walk
// =============================================================================

fn walk(source: &[u8], node: Node, kinds: &LangKinds, state: &mut WalkState, depth: u32) {
    let kind = node.kind();

    // ---- Branch (non-loop) ----------------------------------------------------
    if kinds.branch.contains(&kind) {
        state.branch_count += 1;
        state.cognitive += 1 + depth;
        walk_children(source, node, kinds, state, depth + 1);
        return;
    }

    // ---- Loop -----------------------------------------------------------------
    if kinds.loop_kinds.contains(&kind) {
        state.branch_count += 1;
        state.cognitive += 1 + depth;
        state.in_loop += 1;
        if state.in_loop > state.max_loop_depth {
            state.max_loop_depth = state.in_loop;
        }
        walk_children(source, node, kinds, state, depth + 1);
        state.in_loop -= 1;
        return;
    }

    // ---- Logical operator (`&&` / `||` / `and` / `or`) ------------------------
    if let Some(bool_kind) = kinds.bool_op_kind
        && kind == bool_kind
        && let Some(op_node) = node.child_by_field_name("operator")
        && let Ok(op_text) = op_node.utf8_text(source)
        && kinds.bool_ops.contains(&op_text)
    {
        state.branch_count += 1;
        state.cognitive += 1 + depth;
    }
    // ---- alloc / linear-scan (only meaningful inside a loop) ------------------
    if state.in_loop > 0 {
        if kinds.alloc_match_kinds.contains(&kind)
            && node_text_contains_any(source, node, kinds.alloc_substrings)
        {
            state.alloc_in_loop = true;
        }
        if kinds.scan_match_kinds.contains(&kind)
            && node_text_contains_any(source, node, kinds.scan_substrings)
        {
            state.linear_scan_in_loop = true;
        }
    }

    walk_children(source, node, kinds, state, depth);
}

fn walk_children(source: &[u8], node: Node, kinds: &LangKinds, state: &mut WalkState, depth: u32) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(source, child, kinds, state, depth);
    }
}

fn node_text_contains_any(source: &[u8], node: Node, needles: &[&str]) -> bool {
    let Ok(text) = node.utf8_text(source) else {
        return false;
    };
    needles.iter().any(|n| text.contains(n))
}

// =============================================================================
// Recursion detection
// =============================================================================

fn detect_recursion(
    source: &[u8],
    func_root: Node,
    func_name: &str,
    kinds: &LangKinds,
    state: &mut WalkState,
) {
    let Some(call_line) = find_first_recursive_call(source, func_root, kinds.call_kind, func_name)
    else {
        return;
    };
    state.is_recursive = true;
    if !has_return_guard_before(source, func_root, kinds.if_kind, kinds.return_kinds, call_line) {
        state.unguarded_recursion = true;
    }
}

fn find_first_recursive_call(
    source: &[u8],
    node: Node,
    call_kind: &str,
    func_name: &str,
) -> Option<u32> {
    if node.kind() == call_kind
        && let Some(callee) = node.child_by_field_name("function")
        && let Ok(text) = callee.utf8_text(source)
    {
        // Match by exact name OR by qualified name ending in `func_name`.
        // Catches `self::fib`, `crate::fib`, `Foo::fib`, etc.
        let callee = text.trim();
        if callee == func_name || callee.ends_with(&format!("::{}", func_name)) {
            return Some(node.start_position().row as u32);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(line) = find_first_recursive_call(source, child, call_kind, func_name) {
            return Some(line);
        }
    }
    None
}

fn has_return_guard_before(
    source: &[u8],
    node: Node,
    if_kinds: &[&str],
    return_kinds: &[&str],
    call_line: u32,
) -> bool {
    if if_kinds.contains(&node.kind()) {
        let end_line = node.end_position().row as u32;
        if end_line <= call_line {
            // Guard path 1: the if's body terminates control flow with an
            // explicit return / break / continue.
            if subtree_has_kind(source, node, return_kinds) {
                return true;
            }
            // Guard path 2: the if has an `else` branch and the recursion
            // lives in that branch. Taking the if's body skips the
            // recursive call (e.g. `if n < 2 { n } else { fib(...) }`
            // where the if body is the function's tail expression).
            if has_else_branch(node) {
                // But this only counts as a guard if the if's body itself
                // is the "early exit" — which it is whenever the else
                // contains the first recursive call. The end_line check
                // above already limits us to ifs that complete before
                // the recursive call starts.
                return true;
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_return_guard_before(source, child, if_kinds, return_kinds, call_line) {
            return true;
        }
    }
    false
}

/// Does this if-like node have an `else` / `alternative` branch?
///
/// Tree-sitter exposes the alternate branch through field names like
/// `alternative` (Rust `if_expression`) or `else` (JS/Python) or via a
/// child named `else` / `elif_clause` / `elif`. We probe several common
/// locations and rely on the AST shape of each language.
fn has_else_branch(node: Node) -> bool {
    // Field-based probes — preferred where the grammar wires a named field.
    if node.child_by_field_name("alternative").is_some() {
        return true;
    }
    if node.child_by_field_name("else").is_some() {
        return true;
    }
    // Sibling / child-name probes — covers grammars that don't name the
    // else branch.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "else_clause" | "elif_clause" => return true,
            k if k.starts_with("else") => return true,
            _ => {}
        }
    }
    false
}

fn subtree_has_kind(source: &[u8], node: Node, kinds: &[&str]) -> bool {
    if kinds.contains(&node.kind()) {
        return true;
    }
    let _ = source;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if subtree_has_kind(source, child, kinds) {
            return true;
        }
    }
    false
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_rust(src: &str) -> tree_sitter::Tree {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        p.parse(src, None).expect("parse")
    }

    fn first_func_body<'a>(tree: &'a tree_sitter::Tree) -> tree_sitter::Node<'a> {
        find_first_function(tree.root_node()).expect("function_item")
    }

    fn find_first_function<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
        if node.kind() == "function_item" {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_first_function(child) {
                return Some(found);
            }
        }
        None
    }

    fn run(src: &str) -> ComplexityMetrics {
        let tree = parse_rust(src);
        let func = first_func_body(&tree);
        compute_metrics(src.as_bytes(), func, "f", "rust")
    }

    fn run_named(src: &str, name: &str) -> ComplexityMetrics {
        let tree = parse_rust(src);
        let func = first_func_body(&tree);
        compute_metrics(src.as_bytes(), func, name, "rust")
    }

    #[test]
    fn linear_no_branches() {
        let m = run("fn f() {}");
        assert_eq!(m.cyclomatic, 1);
        assert_eq!(m.cognitive, 0);
        assert_eq!(m.max_loop_depth, 0);
        assert!(!m.alloc_in_loop);
        assert!(!m.linear_scan_in_loop);
        assert!(!m.is_recursive);
        assert!(!m.unguarded_recursion);
    }

    #[test]
    fn cyclomatic_counts_if() {
        let m = run("fn f() { if x {} if y {} }");
        // Two top-level `if`s on the same nesting level. With standard
        // cognitive-complexity semantics (depth increments inside the body of
        // a control structure and decrements after), each contributes
        // (1 + 0) = 1, total 2.
        //
        // NOTE: the original brief states `cognitive=4` with the
        // parenthetical math `(1+0) + (1+1) = 3` — those two assertions are
        // mutually inconsistent, and 4 is not reachable from this AST under
        // any standard formula. We assert 2 here, which is the SonarSource
        // canonical value for this snippet.
        assert_eq!(m.cyclomatic, 3);
        assert_eq!(m.cognitive, 2);
    }

    #[test]
    fn nested_increments_cognitive() {
        // Outer if at depth 0 contributes 1; inner if at depth 1 contributes 2.
        let m = run("fn f() { if x { if y { } } }");
        assert_eq!(m.cognitive, 3);
    }

    #[test]
    fn max_loop_depth() {
        let m = run("fn f() { for _ in 0..1 { for _ in 0..1 {} } }");
        assert_eq!(m.max_loop_depth, 2);
    }

    #[test]
    fn alloc_in_loop() {
        let m = run("fn f() { for _ in 0..1 { let v = vec![1]; } }");
        assert!(m.alloc_in_loop);
    }

    #[test]
    fn linear_scan_in_loop() {
        let m = run("fn f(v: &Vec<i32>) { for _ in 0..1 { v.contains(&1); } }");
        assert!(m.linear_scan_in_loop);
    }

    #[test]
    fn recursion_detected() {
        let m = run_named(
            "fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n-1) + fib(n-2) } }",
            "fib",
        );
        assert!(m.is_recursive);
        assert!(!m.unguarded_recursion);
    }

    #[test]
    fn unguarded_recursion() {
        let m = run_named("fn loop_f() { loop_f(); }", "loop_f");
        assert!(m.is_recursive);
        assert!(m.unguarded_recursion);
    }

    #[test]
    fn extra_roundtrip() {
        let m = ComplexityMetrics {
            cyclomatic: 7,
            cognitive: 11,
            max_loop_depth: 3,
            alloc_in_loop: true,
            linear_scan_in_loop: false,
            is_recursive: true,
            unguarded_recursion: false,
        };
        let extra = metrics_to_extra(&m);
        let back = metrics_from_extra(&extra).expect("roundtrip");
        assert_eq!(m, back);
    }

    #[test]
    fn max_loop_depth_unaffected_by_nested_ifs() {
        let m = run("fn f() { if x { if y { for _ in 0..1 {} } } }");
        // The single `for` is nested inside two ifs, but only loops count
        // toward `max_loop_depth`.
        assert_eq!(m.max_loop_depth, 1);
    }
}