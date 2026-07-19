//! Walk the parsed AST (or, when no AST is available — e.g. for unit tests
//! feeding raw source — the source bytes alone) and emit [`RouteDefinition`]s
//! plus [`CallSite`]s via the [`patterns`] module.
//!
//! The integration entry point consumed by `engine::tree_sitter::parse_file`
//! is [`extract_routes_and_calls`]. It returns an [`ExtractResult`] the
//! caller turns into `NodeKind::Route` graph nodes plus `route_handler` /
//! `http_calls` edges.

use crate::engine::http::patterns::{detect_calls, detect_routes, CallMatch, RouteMatch};

/// A route definition in the AST, ready to be promoted to a `Node` with
/// `kind = NodeKind::Route` and `role = "server"`. The handler QN is `Some`
/// only when the matcher could resolve it (Express handler argument,
/// decorator-followed-by-`def`, `#[actix]`-followed-by-`fn`).
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDefinition {
    pub method: String,
    pub path: String,
    pub handler_qn: Option<String>,
    pub framework: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: usize,
}

impl RouteDefinition {
    /// Always `"server"` — call sites use [`CallSite`] instead. Kept as a
    /// method so the integrator can build stable IDs without branching on
    /// the variant type.
    pub fn role_label(&self) -> &'static str {
        "server"
    }
}

/// An outbound HTTP call site in the AST, ready to be promoted to a `Node`
/// with `kind = NodeKind::Route` and `role = "client"`. `caller_qn` is the
/// qualified name of the enclosing function (resolved by the integrator
/// after the regular definition-arm pass emits it).
#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    pub method: String,
    pub path: String,
    pub framework: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: usize,
}

impl CallSite {
    pub fn role_label(&self) -> &'static str {
        "client"
    }
}

/// Combined return value of [`extract_routes_and_calls`].
#[derive(Debug, Clone, Default)]
pub struct ExtractResult {
    pub routes: Vec<RouteDefinition>,
    pub calls: Vec<CallSite>,
}

/// Walk the parsed source and emit routes + call sites.
///
/// `root` is accepted for forward compatibility with a future AST-aware
/// integration (the prompt mentions it as a parameter); v1 uses textual
/// scanning via [`patterns`] so the same call site works in tests that
/// pass raw bytes.
pub fn extract_routes_and_calls(
    source: &[u8],
    _root: Option<tree_sitter::Node>,
    _lang: &str,
) -> ExtractResult {
    let text = match std::str::from_utf8(source) {
        Ok(s) => s,
        Err(_) => return ExtractResult::default(),
    };
    let raw_routes = detect_routes(text);
    let raw_calls = detect_calls(text);

    ExtractResult {
        routes: raw_routes
            .into_iter()
            .map(route_match_to_definition)
            .collect(),
        calls: raw_calls.into_iter().map(call_match_to_site).collect(),
    }
}

fn route_match_to_definition(m: RouteMatch) -> RouteDefinition {
    RouteDefinition {
        method: m.method,
        path: m.path,
        handler_qn: m.handler_qn,
        framework: m.framework.to_string(),
        start_line: m.start_line,
        end_line: m.end_line,
        start_byte: m.start_byte,
    }
}

fn call_match_to_site(m: CallMatch) -> CallSite {
    CallSite {
        method: m.method,
        path: m.path,
        framework: m.framework.to_string(),
        start_line: m.start_line,
        end_line: m.end_line,
        start_byte: m.start_byte,
    }
}

/// Sanitize a route path for use as part of a node ID. Replaces `/` and
/// other URL-meaningful characters that would otherwise make the ID
/// collide with the graph's other ID conventions.
pub(crate) fn sanitize_id_segment(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' => '_',
            ':' | '{' | '}' | '<' | '>' | '*' | '?' | ' ' => '_',
            _ => c,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Internal tests — the cross-language happy-path coverage lives here. The
// cross-service + normalize-path tests live in `crate::types` (so they're
// picked up by the engine-level test runner too) and the integration tests
// for `tree_sitter.rs` live in `engine::tree_sitter::tests`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod pattern_tests {
    use super::*;

    fn first_route(source: &str) -> RouteDefinition {
        let r = extract_routes_and_calls(source.as_bytes(), None, "typescript");
        assert_eq!(r.routes.len(), 1, "expected 1 route, got {}: {:?}", r.routes.len(), r.routes);
        r.routes.into_iter().next().unwrap()
    }

    #[test]
    fn express_basic() {
        let r = first_route("app.get('/users/:id', handler)");
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/users/:id");
        assert_eq!(r.handler_qn.as_deref(), Some("handler"));
        assert_eq!(r.framework, "express");
    }

    #[test]
    fn fastapi_decorator() {
        let src = "@app.get('/items/{item_id}')\nasync def read_item(item_id): pass\n";
        let r = first_route(src);
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/items/{item_id}");
        assert_eq!(r.handler_qn.as_deref(), Some("read_item"));
    }

    #[test]
    fn gin_chi() {
        let r = first_route("r.GET(\"/users\", handler)");
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/users");
        assert_eq!(r.handler_qn.as_deref(), Some("handler"));
    }

    #[test]
    fn actix_proc_macro() {
        let src = "#[get(\"/hello\")] async fn hi() {}\n";
        let r = first_route(src);
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/hello");
        assert_eq!(r.handler_qn.as_deref(), Some("hi"));
    }

    #[test]
    fn fetch_call_extracts_path() {
        let r = extract_routes_and_calls(b"fetch('/users/42')", None, "javascript");
        assert_eq!(r.calls.len(), 1);
        let c = &r.calls[0];
        assert_eq!(c.method, "GET");
        assert_eq!(c.path, "/users/42");
        assert_eq!(c.framework, "fetch");
    }

    #[test]
    fn axios_post_call() {
        let r = extract_routes_and_calls(b"axios.post('/orders', body)", None, "javascript");
        assert_eq!(r.calls.len(), 1);
        let c = &r.calls[0];
        assert_eq!(c.method, "POST");
        assert_eq!(c.path, "/orders");
        assert_eq!(c.framework, "axios");
    }
}