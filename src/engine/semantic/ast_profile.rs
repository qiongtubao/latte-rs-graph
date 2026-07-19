//! AST-derived semantic profile for function subtrees.
//!
//! The profile intentionally uses node kinds instead of grammar queries.  The
//! grammars used by the engine have a small, stable vocabulary for the
//! constructs represented here, while the fallback cases keep this useful for
//! parser error nodes and newer grammar revisions.

use tree_sitter::Node;

const PROFILE_LEN: usize = 25;

#[derive(Clone, Copy)]
enum Language {
    Rust,
    JavaScript,
    C,
    Generic,
}

#[derive(Clone, Copy)]
enum Control {
    If,
    Match,
    Loop,
    Try,
}

#[derive(Clone, Copy)]
enum Literal {
    Integer,
    Float,
    String,
    Other,
}

struct ProfileState<'a> {
    counts: [u64; 16],
    operator_count: u64,
    operand_count: u64,
    operator_vocabulary: Vec<&'a [u8]>,
    operand_vocabulary: Vec<&'a [u8]>,
    nesting_count: u64,
    nesting_total: f64,
    nesting_square_total: f64,
    nesting_max: u32,
}

impl<'a> ProfileState<'a> {
    fn new() -> Self {
        Self {
            counts: [0; 16],
            operator_count: 0,
            operand_count: 0,
            // Entries borrow source slices, so vocabulary tracking never
            // copies identifiers, literals, or operator text.
            operator_vocabulary: Vec::new(),
            operand_vocabulary: Vec::new(),
            nesting_count: 0,
            nesting_total: 0.0,
            nesting_square_total: 0.0,
            nesting_max: 0,
        }
    }

    fn increment(&mut self, index: usize) {
        if let Some(value) = self.counts.get_mut(index) {
            *value = value.saturating_add(1);
        }
    }

    fn record_nesting(&mut self, depth: u32) {
        self.nesting_count = self.nesting_count.saturating_add(1);
        let depth = f64::from(depth);
        self.nesting_total += depth;
        self.nesting_square_total += depth * depth;
        self.nesting_max = self.nesting_max.max(depth as u32);
    }

    fn add_operator(&mut self, lexeme: &'a [u8]) {
        self.operator_count = self.operator_count.saturating_add(1);
        add_distinct(&mut self.operator_vocabulary, lexeme);
    }

    fn add_operand(&mut self, lexeme: &'a [u8]) {
        self.operand_count = self.operand_count.saturating_add(1);
        add_distinct(&mut self.operand_vocabulary, lexeme);
    }
}

/// Compute the 25-dimensional AST profile for `func_root`.
///
/// `func_root` is deliberately the sole traversal root: callers can pass a
/// function item, method definition, or a body node without accidentally
/// incorporating neighbouring declarations.  All text access is bounds
/// checked against `source`, so error and empty trees are harmless.
pub fn compute_ast_profile(source: &[u8], func_root: Node, lang: &str) -> Vec<f32> {
    let language = language_for(lang);
    let mut parameter_names = Vec::new();
    collect_parameter_names(source, func_root, &mut parameter_names);

    let mut state = ProfileState::new();
    walk(
        source,
        func_root,
        language,
        &parameter_names,
        &mut state,
        0,
        false,
        false,
        false,
    );

    let (mean, standard_deviation) = if state.nesting_count == 0 {
        (0.0, 0.0)
    } else {
        let count = state.nesting_count as f64;
        let mean = state.nesting_total / count;
        // Roundoff can make a mathematically zero variance very slightly
        // negative.  Clamping also protects sqrt from malformed arithmetic.
        let variance = (state.nesting_square_total / count - mean * mean).max(0.0);
        (mean, variance.sqrt())
    };

    let vocabulary = state
        .operator_vocabulary
        .len()
        .saturating_add(state.operand_vocabulary.len()) as f64;
    let length = state.operator_count.saturating_add(state.operand_count) as f64;
    let loc = lines_of_node(source, func_root) as f64;

    let mut profile = vec![0.0; PROFILE_LEN];
    profile[0] = finite_f32(state.counts[0] as f64);
    profile[1] = finite_f32(state.counts[1] as f64);
    profile[2] = finite_f32(state.counts[2] as f64);
    profile[3] = finite_f32(state.counts[3] as f64);
    profile[4] = finite_f32(state.nesting_max as f64);
    profile[5] = finite_f32(mean);
    profile[6] = finite_f32(state.nesting_total);
    profile[7] = finite_f32(standard_deviation);
    profile[8] = finite_f32(state.counts[4] as f64);
    profile[9] = finite_f32(state.counts[5] as f64);
    profile[10] = finite_f32(state.counts[6] as f64);
    profile[11] = finite_f32(state.counts[7] as f64);
    profile[12] = finite_f32(state.counts[8] as f64);
    profile[13] = finite_f32(state.counts[9] as f64);
    profile[14] = finite_f32(state.counts[10] as f64);
    profile[15] = finite_f32(state.counts[11] as f64);
    profile[16] = finite_f32(state.counts[12] as f64);
    profile[17] = finite_f32(state.counts[13] as f64);
    profile[18] = finite_f32(state.counts[14] as f64);
    profile[19] = finite_f32(state.counts[15] as f64);
    profile[20] = finite_f32(state.operator_count as f64);
    profile[21] = finite_f32(state.operand_count as f64);
    profile[22] = finite_f32(vocabulary);
    profile[23] = finite_f32(length);
    profile[24] = finite_f32(loc);
    profile
}

fn language_for(lang: &str) -> Language {
    if lang.eq_ignore_ascii_case("rust") || lang.eq_ignore_ascii_case("rs") {
        Language::Rust
    } else if lang.eq_ignore_ascii_case("typescript")
        || lang.eq_ignore_ascii_case("ts")
        || lang.eq_ignore_ascii_case("javascript")
        || lang.eq_ignore_ascii_case("js")
    {
        Language::JavaScript
    } else if lang.eq_ignore_ascii_case("c")
        || lang.eq_ignore_ascii_case("cpp")
        || lang.eq_ignore_ascii_case("c++")
        || lang.eq_ignore_ascii_case("cc")
    {
        Language::C
    } else {
        Language::Generic
    }
}

fn walk<'a, 'tree>(
    source: &'a [u8],
    node: Node<'tree>,
    language: Language,
    parameter_names: &[&'a [u8]],
    state: &mut ProfileState<'a>,
    nesting: u32,
    in_arguments: bool,
    in_parameters: bool,
    in_literal: bool,
) {
    let kind = node.kind();
    let control = control_kind(kind, language);
    let control_nesting = if control.is_some() {
        let depth = nesting.saturating_add(1);
        state.record_nesting(depth);
        depth
    } else {
        nesting
    };

    if let Some(control) = control {
        state.increment(match control {
            Control::If => 0,
            Control::Match => 1,
            Control::Loop => 2,
            Control::Try => 3,
        });
        // Control constructs are operators in the Halstead-lite tally.  The
        // grammar kind is a stable fallback when there is no operator field.
        state.add_operator(kind.as_bytes());
    }

    let argument_context = in_arguments || is_argument_container(kind);
    let parameter_context = in_parameters || is_parameter_node(kind);
    let literal = literal_kind(source, node);
    let literal_context = in_literal || literal.is_some();

    if is_binary_kind(kind) {
        state.increment(4);
        add_node_operator(source, node, state, b"binary");
    } else if is_unary_kind(kind) {
        state.increment(5);
        add_node_operator(source, node, state, b"unary");
    }

    if kind == "call_expression" || kind == "call" || kind == "call_statement" {
        state.increment(6);
        state.add_operator(b"call");
    }
    if is_member_kind(kind) {
        state.increment(7);
        state.add_operator(b"member");
    }

    if let Some(literal) = literal {
        match literal {
            Literal::Integer => state.increment(8),
            Literal::Float => state.increment(9),
            Literal::String => state.increment(10),
            Literal::Other => {}
        }
        state.add_operand(node_text(source, node).unwrap_or(kind.as_bytes()));
    } else if is_identifier_kind(kind) {
        let text = node_text(source, node).unwrap_or(kind.as_bytes());
        state.add_operand(text);
        if literal_context {
            state.increment(11);
        }
        if argument_context {
            state.increment(15);
        }
        if !parameter_context && parameter_names.iter().any(|name| *name == text) {
            state.increment(14);
        }
    }

    if is_assignment_kind(kind) {
        state.increment(12);
        add_node_operator(source, node, state, b"=");
    }
    if kind == "return_expression" || kind == "return_statement" {
        state.increment(13);
        state.add_operator(b"return");
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(
            source,
            child,
            language,
            parameter_names,
            state,
            control_nesting,
            argument_context,
            parameter_context,
            literal_context,
        );
    }
}

fn collect_parameter_names<'a, 'tree>(
    source: &'a [u8],
    node: Node<'tree>,
    names: &mut Vec<&'a [u8]>,
) {
    if is_parameter_node(node.kind()) {
        collect_parameter_bindings(source, node, names);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_parameter_names(source, child, names);
    }
}

fn collect_parameter_bindings<'a, 'tree>(
    source: &'a [u8],
    node: Node<'tree>,
    names: &mut Vec<&'a [u8]>,
) {
    if node.kind() == "identifier" {
        if let Some(text) = node_text(source, node) {
            if !names.iter().any(|name| *name == text) {
                names.push(text);
            }
        }
        return;
    }

    if node.kind() == "assignment_pattern" {
        if let Some(left) = node.child_by_field_name("left") {
            collect_parameter_bindings(source, left, names);
        }
        return;
    }

    // Rust and TypeScript expose a pattern; C and C++ expose a declarator.
    // JavaScript/TypeScript simple parameters commonly use a `name` field.
    for field in ["pattern", "declarator", "name"] {
        if let Some(child) = node.child_by_field_name(field) {
            collect_parameter_bindings(source, child, names);
            return;
        }
    }

    // Containers (parameters, formal_parameters, and parameter_list) have no
    // binding field of their own.  Their direct children are parameter nodes.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_parameter_bindings(source, child, names);
    }
}

fn add_node_operator<'a, 'tree>(
    source: &'a [u8],
    node: Node<'tree>,
    state: &mut ProfileState<'a>,
    fallback: &'a [u8],
) {
    let operator = node
        .child_by_field_name("operator")
        .and_then(|child| node_text(source, child))
        .unwrap_or(fallback);
    state.add_operator(operator);
}

fn add_distinct<'a>(values: &mut Vec<&'a [u8]>, value: &'a [u8]) {
    if !values.iter().any(|candidate| *candidate == value) {
        values.push(value);
    }
}

fn node_text<'a, 'tree>(source: &'a [u8], node: Node<'tree>) -> Option<&'a [u8]> {
    let range = node.byte_range();
    if range.start <= range.end && range.end <= source.len() {
        Some(&source[range.start..range.end])
    } else {
        None
    }
}

fn lines_of_node(source: &[u8], node: Node<'_>) -> u64 {
    let range = node.byte_range();
    if range.start >= range.end || range.end > source.len() {
        return 0;
    }
    let start = node.start_position().row;
    let end = node.end_position().row;
    u64::try_from(end.saturating_sub(start).saturating_add(1)).unwrap_or(u64::MAX)
}

fn finite_f32(value: f64) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    value.min(f64::from(f32::MAX)).max(f64::from(f32::MIN)) as f32
}

fn control_kind(kind: &str, language: Language) -> Option<Control> {
    let rust = matches!(
        kind,
        "if_expression"
            | "if_let_expression"
            | "match_expression"
            | "loop_expression"
            | "for_expression"
            | "while_expression"
            | "try_expression"
            | "try_block"
    );
    let js = matches!(
        kind,
        "if_statement"
            | "ternary_expression"
            | "switch_statement"
            | "switch_expression"
            | "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "while_statement"
            | "do_statement"
            | "try_statement"
    );
    let c = matches!(
        kind,
        "if_statement"
            | "conditional_expression"
            | "switch_statement"
            | "switch_expression"
            | "for_statement"
            | "for_range_loop"
            | "range_based_for_statement"
            | "while_statement"
            | "do_statement"
            | "try_statement"
    );

    let matches_language = match language {
        Language::Rust => rust,
        Language::JavaScript => js,
        Language::C => c,
        Language::Generic => rust || js || c,
    };
    if !matches_language {
        return None;
    }

    if matches!(
        kind,
        "if_expression"
            | "if_let_expression"
            | "if_statement"
            | "ternary_expression"
            | "conditional_expression"
    ) {
        Some(Control::If)
    } else if matches!(
        kind,
        "match_expression" | "switch_statement" | "switch_expression"
    ) {
        Some(Control::Match)
    } else if matches!(
        kind,
        "loop_expression"
            | "for_expression"
            | "for_statement"
            | "for_in_statement"
            | "for_of_statement"
            | "for_range_loop"
            | "range_based_for_statement"
            | "while_expression"
            | "while_statement"
            | "do_statement"
    ) {
        Some(Control::Loop)
    } else {
        Some(Control::Try)
    }
}

fn is_binary_kind(kind: &str) -> bool {
    matches!(
        kind,
        "binary_expression" | "logical_expression" | "boolean_operator"
    )
}

fn is_unary_kind(kind: &str) -> bool {
    matches!(
        kind,
        "unary_expression"
            | "reference_expression"
            | "dereference_expression"
            | "pointer_expression"
            | "sizeof_expression"
            | "delete_expression"
    )
}

fn is_member_kind(kind: &str) -> bool {
    matches!(
        kind,
        "field_expression" | "member_expression" | "subscript_expression" | "index_expression"
    )
}

fn is_assignment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "assignment_expression"
            | "assignment_statement"
            | "augmented_assignment_expression"
            | "compound_assignment_expression"
            | "assignment_pattern"
    )
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "field_identifier"
            | "property_identifier"
            | "private_property_identifier"
            | "type_identifier"
    )
}

fn is_integer_kind(kind: &str) -> bool {
    matches!(
        kind,
        "integer_literal"
            | "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal"
    )
}

fn is_float_kind(kind: &str) -> bool {
    matches!(
        kind,
        "float_literal" | "decimal_floating_point_literal" | "float"
    )
}

fn literal_kind<'a, 'tree>(source: &'a [u8], node: Node<'tree>) -> Option<Literal> {
    let kind = node.kind();
    if is_integer_kind(kind) {
        Some(Literal::Integer)
    } else if is_float_kind(kind) {
        Some(Literal::Float)
    } else if matches!(kind, "number" | "number_literal") {
        let text = node_text(source, node).unwrap_or_default();
        Some(if numeric_text_is_float(text) {
            Literal::Float
        } else {
            Literal::Integer
        })
    } else if is_string_kind(kind) {
        Some(Literal::String)
    } else if is_other_literal_kind(kind) {
        Some(Literal::Other)
    } else {
        None
    }
}

fn numeric_text_is_float(text: &[u8]) -> bool {
    let mut start = 0;
    while start < text.len() && (text[start] == b'+' || text[start] == b'-') {
        start += 1;
    }
    let is_hex = text.get(start..).is_some_and(|rest| {
        rest.len() >= 2 && rest[0] == b'0' && (rest[1] == b'x' || rest[1] == b'X')
    });
    if text.contains(&b'.') || text.contains(&b'p') || text.contains(&b'P') {
        return true;
    }
    if !is_hex && (text.contains(&b'e') || text.contains(&b'E')) {
        return true;
    }
    !is_hex
        && text
            .last()
            .is_some_and(|last| *last == b'f' || *last == b'F')
}

fn is_string_kind(kind: &str) -> bool {
    matches!(
        kind,
        "string_literal" | "raw_string_literal" | "string" | "template_string" | "template_literal"
    )
}

fn is_other_literal_kind(kind: &str) -> bool {
    matches!(
        kind,
        "boolean_literal"
            | "char_literal"
            | "regex"
            | "null"
            | "null_literal"
            | "undefined"
            | "true"
            | "false"
            | "negative_literal"
            | "user_defined_literal"
    )
}

fn is_argument_container(kind: &str) -> bool {
    matches!(
        kind,
        "argument_list" | "arguments" | "call_arguments" | "subscript_argument_list"
    )
}

fn is_parameter_node(kind: &str) -> bool {
    matches!(
        kind,
        "parameter"
            | "parameter_declaration"
            | "parameters"
            | "parameter_list"
            | "formal_parameters"
            | "required_parameter"
            | "optional_parameter"
            | "rest_pattern"
    )
}
