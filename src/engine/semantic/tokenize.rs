//! Deterministic identifier tokenization over a tree-sitter subtree.
//!
//! `semantic_tokens` walks the given node and emits a deduplicated, lowercased
//! sequence of the identifier- and type-like leaves it finds. The crate-internal
//! helpers [`data_flow_tokens`] and [`api_signature`] apply the same machinery
//! but only emit leaves that sit under an assignment/return/argument ancestor or
//! a parameter/type/decorator ancestor respectively. No regex, no
//! dependencies beyond `tree-sitter` itself.

use std::collections::HashSet;
use tree_sitter::Node;

/// Lower-cased, deduplicated identifier leaves under `node`.
///
/// Iterates the subtree rooted at `node`, collecting every named leaf whose
/// kind is `identifier`, `type_identifier`, or `primitive_type`. Tokens are
/// normalized to ASCII lower-case, language-agnostic code stopwords are
/// dropped, and duplicates are removed while preserving the order of first
/// occurrence. Leaves whose source bytes are not valid UTF-8 are skipped.
pub fn semantic_tokens(source: &[u8], node: Node) -> Vec<String> {
    collect_tokens(source, node, Context::Any)
}

/// Identifier leaves under assignment / return / argument-like contexts.
///
/// Useful for capturing the data flow of a callable: variables introduced by
/// `let` bindings, the targets and sources of assignment expressions, values
/// returned by `return`, and arguments passed to call expressions.
pub(crate) fn data_flow_tokens(source: &[u8], node: Node) -> Vec<String> {
    collect_tokens(source, node, Context::DataFlow)
}

/// Identifier and type-identifier leaves under parameter / type / decorator-like
/// ancestors.
///
/// Useful for capturing the API surface of a callable: function name,
/// parameter names, parameter and return types, and attributes.
pub(crate) fn api_signature(source: &[u8], node: Node) -> Vec<String> {
    collect_tokens(source, node, Context::ApiSignature)
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/// Which leaves to emit and which ancestor predicates must hold.
#[derive(Clone, Copy)]
enum Context {
    /// Emit every identifier-like leaf under the subtree.
    Any,
    /// Emit identifier-like leaves under assignment/return/argument ancestors.
    ///
    /// Only `identifier` leaves are kept — types are not data flow.
    DataFlow,
    /// Emit identifier / type-identifier leaves under parameter / type /
    /// decorator ancestors.
    ApiSignature,
}

/// Language-agnostic stopwords filtered after lower-casing.
///
/// Includes Rust control-flow / declaration keywords plus primitive scalar
/// type names that tree-sitter emits as `primitive_type` leaves. Many of these
/// never reach us as identifier-like leaves in practice; listing them keeps
/// the filter defensive against grammars where the same surface text appears
/// as an identifier (e.g. `int` in C-like languages).
const STOPWORDS: &[&str] = &[
    // control / declaration keywords
    "fn",
    "let",
    "mut",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "match",
    "return",
    "in",
    "as",
    "ref",
    "use",
    "pub",
    "mod",
    "struct",
    "enum",
    "trait",
    "impl",
    "self",
    "super",
    "crate",
    "where",
    "async",
    "await",
    "dyn",
    "move",
    "do",
    "yield",
    "break",
    "continue",
    "goto",
    "switch",
    "case",
    "default",
    "try",
    "catch",
    "throw",
    "throws",
    "finally",
    "new",
    "class",
    "interface",
    "extends",
    "implements",
    "package",
    "import",
    "export",
    "from",
    "with",
    "of",
    "is",
    "not",
    "and",
    "or",
    "void",
    "const",
    "static",
    "public",
    "private",
    "protected",
    "internal",
    // primitive scalar types
    "int",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "uint",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "f32",
    "f64",
    "bool",
    "char",
    "str",
    "string",
    // common literal / value names
    "true",
    "false",
    "none",
    "null",
    "nil",
];

/// Ancestor kinds that mark a `data_flow_tokens` context.
const DATA_FLOW_ANCESTORS: &[&str] = &[
    "assignment_expression",
    "return_expression",
    "arguments",
    "let_declaration",
    "for_expression",
    "while_expression",
    "loop_expression",
    "call_expression",
];

/// Ancestor kinds that mark an `api_signature` context.
const API_SIGNATURE_ANCESTORS: &[&str] = &[
    "attribute_item",
    "inner_attribute_item",
    "outer_attribute_item",
    "attribute",
    "decorator",
    "return_type",
    "type_identifier",
    "generic_type",
    "type_arguments",
    "type_parameters",
    "array_type",
    "reference_type",
    "pointer_type",
    "function_type",
    "tuple_type",
    "unit_type",
    "bounded_type",
    "dynamic_type",
    "optional_type_parameter",
    "scoped_identifier",
    "scoped_type_identifier",
    "qualified_identifier",
    "qualified_type_identifier",
];

/// Whether `kind` is an identifier-like leaf we want to consider for emission.
fn is_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "primitive_type"
            | "field_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            | "scoped_identifier"
            | "scoped_type_identifier"
            | "qualified_identifier"
            | "qualified_type_identifier"
    )
}

/// Whether `kind` is an identifier- or type-identifier-like leaf (excludes
/// raw primitive scalar names). Used for the API signature channel.
fn is_identifier_or_type(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "primitive_type"
            | "predefined_type"
            | "scoped_identifier"
            | "scoped_type_identifier"
            | "qualified_identifier"
            | "qualified_type_identifier"
    )
}

/// Whether `kind` marks an assignment/return/argument-style data-flow context.
fn is_data_flow_ancestor(kind: &str) -> bool {
    DATA_FLOW_ANCESTORS.contains(&kind)
}

/// Whether `kind` marks a parameter/type/decorator-style API-signature context.
fn is_api_signature_ancestor(kind: &str) -> bool {
    API_SIGNATURE_ANCESTORS.contains(&kind)
}

/// Case-insensitive stopword check. Input is assumed already lower-cased.
fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// Normalize a token by trimming a leading `r#` raw-identifier marker (Rust)
/// and ASCII-lowercasing it. Returns `None` for empty / pure-underscore inputs.
fn normalize_token(raw: &str, filter_stopwords: bool) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed
        .strip_prefix("r#")
        .map(|rest| rest.as_ref())
        .unwrap_or(trimmed)
        .trim_start_matches('_');
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if filter_stopwords && is_stopword(&lowered) {
        return None;
    }
    Some(lowered)
}

/// Walk `root` iteratively with a tree cursor, maintaining an ancestor stack,
/// and emit the desired leaves into `tokens`.
fn collect_tokens(source: &[u8], root: Node, context: Context) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cursor = root.walk();
    // Ancestor stack mirrors the cursor's depth minus one: it contains the
    // kinds of every node above the cursor's current position. Pushed on
    // descent, popped on ascent.
    let mut ancestors: Vec<&str> = Vec::new();
    let mut finished = false;

    while !finished {
        let node = cursor.node();
        let kind = node.kind();
        emit_if_match(
            source,
            node,
            kind,
            context,
            &ancestors,
            &mut seen,
            &mut tokens,
        );

        if cursor.goto_first_child() {
            ancestors.push(kind);
            continue;
        }

        // No children — try to advance to a sibling or ascend.
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                finished = true;
                break;
            }
            ancestors.pop();
        }
    }

    tokens
}

/// Decide whether `node` is a leaf the current `context` wants to emit, and if
/// so append its normalized, deduplicated form to `tokens`.
#[allow(clippy::too_many_arguments)]
fn emit_if_match(
    source: &[u8],
    node: Node,
    kind: &str,
    context: Context,
    ancestors: &[&str],
    seen: &mut HashSet<String>,
    tokens: &mut Vec<String>,
) {
    // Only leaves carry source text worth tokenizing; interior nodes are
    // already represented by their descendant leaves.
    if node.child_count() != 0 {
        return;
    }

    match context {
        Context::Any => {
            if !is_identifier_like(kind) {
                return;
            }
        }
        Context::DataFlow => {
            if kind != "identifier" {
                return;
            }
            if !ancestors.iter().any(|k| is_data_flow_ancestor(k)) {
                return;
            }
        }
        Context::ApiSignature => {
            if !is_identifier_or_type(kind) {
                return;
            }
            let is_type_leaf = matches!(
                kind,
                "type_identifier"
                    | "primitive_type"
                    | "predefined_type"
                    | "scoped_type_identifier"
                    | "qualified_type_identifier"
            );
            if !is_type_leaf && !ancestors.iter().any(|k| is_api_signature_ancestor(k)) {
                return;
            }
        }
    }

    // Source bytes may not be valid UTF-8 at the node's span; drop the leaf
    // rather than panic on the conversion.
    let text = match node.utf8_text(source) {
        Ok(text) => text,
        Err(_) => return,
    };

    let token = match normalize_token(text, !matches!(context, Context::ApiSignature)) {
        Some(token) => token,
        None => return,
    };

    if seen.insert(token.clone()) {
        tokens.push(token);
    }
}
