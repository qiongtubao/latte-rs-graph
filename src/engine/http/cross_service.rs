//! Cross-service route/call-site matching (Phase 11).
//!
//! Given a set of server-side routes from project A and a set of client-side
//! HTTP calls from project B, score every (A, B) pair and emit the ones
//! that look like they could represent the same logical endpoint.
//!
//! Confidence scoring is intentionally simple:
//!
//! | score | meaning                                              |
//! |-------|------------------------------------------------------|
//! | 1.00  | exact method + exact normalized path                 |
//! | 0.85  | exact method + path with template params matching    |
//! | 0.70  | same method + path matches under a glob              |
//! | 0.50  | methods differ but paths align                        |
//!
//! Anything below `0.50` is filtered out by [`cross_service_candidates`].
//!
//! The cross-service matcher operates on the already-stored `RouteRecord`
//! shape (used by the CLI); it does NOT depend on tree-sitter directly.

use crate::types::{normalize_http_path, path_segments, CrossServiceCandidate, RouteRecord};

/// Convenience input alias. The CLI builds two of these (one per database)
/// and passes them into [`cross_service_candidates`].
pub struct CrossServiceInput<'a> {
    pub project: &'a str,
    pub routes: &'a [RouteRecord],
}

/// Pair every server route in A with every client call in B, scoring each
/// pair with [`confidence_score`] and returning the ones that pass the
/// minimum-confidence filter.
pub fn cross_service_candidates(
    a: CrossServiceInput<'_>,
    b: CrossServiceInput<'_>,
    min_confidence: f32,
) -> Vec<CrossServiceCandidate> {
    let mut out = Vec::new();
    for server in a.routes.iter().filter(|r| r.role == "server") {
        for client in b.routes.iter().filter(|r| r.role == "client") {
            let conf = confidence_score(&server.method, &server.path, &client.method, &client.path);
            if conf >= min_confidence {
                out.push(CrossServiceCandidate {
                    server_path: server.path.clone(),
                    server_method: server.method.clone(),
                    server_file: server.file_path.clone(),
                    server_line: server.start_line,
                    server_framework: server.framework.clone(),
                    client_path: client.path.clone(),
                    client_method: client.method.clone(),
                    client_file: client.file_path.clone(),
                    client_line: client.start_line,
                    client_framework: client.framework.clone(),
                    confidence: conf,
                });
            }
        }
    }
    // Highest confidence first; stable secondary sort by (server, client)
    // path so the CLI output is deterministic across runs.
    out.sort_by(|x, y| {
        y.confidence
            .partial_cmp(&x.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.server_path.cmp(&y.server_path))
            .then_with(|| x.client_path.cmp(&y.client_path))
    });
    let _ = (a.project, b.project); // reserved for future annotations
    out
}

/// Score a (method_a, path_a, method_b, path_b) tuple on a 1.0 / 0.85 /
/// 0.70 / 0.50 scale. Returns 0.0 when nothing aligns; callers filter on
/// a minimum-confidence threshold.
pub fn confidence_score(method_a: &str, path_a: &str, method_b: &str, path_b: &str) -> f32 {
    let norm_a = normalize_http_path(path_a);
    let norm_b = normalize_http_path(path_b);
    let segs_a = path_segments(path_a);
    let segs_b = path_segments(path_b);

    // 1) exact method + exact path (after normalize)
    let method_match = method_a.eq_ignore_ascii_case(method_b);
    if method_match && norm_a == norm_b {
        return 1.00;
    }

    // 2) exact method + path with template params matching
    //    /users/* ↔ /users/42, /users/<id> ↔ /users/abc
    if method_match && segs_a.len() == segs_b.len() {
        let mut aligned = true;
        for (sa, sb) in segs_a.iter().zip(segs_b.iter()) {
            if *sa == "*" || *sb == "*" {
                continue;
            }
            if sa != sb {
                aligned = false;
                break;
            }
        }
        if aligned {
            return 0.85;
        }
    }

    // 3) same method + glob (one side ends with /* and the other matches
    //    the prefix); /users/* ↔ /users
    if method_match && (norm_a.ends_with("/*") || norm_b.ends_with("/*")) {
        let prefix_a = norm_a.trim_end_matches("/*");
        let prefix_b = norm_b.trim_end_matches("/*");
        if prefix_a == prefix_b && !prefix_a.is_empty() {
            return 0.70;
        }
    }

    // 4) methods differ but paths align structurally
    if segs_a == segs_b && !method_match {
        return 0.50;
    }

    0.0
}

#[cfg(test)]
mod cross_service_tests {
    use super::*;

    fn server(method: &str, path: &str) -> RouteRecord {
        RouteRecord {
            method: method.into(),
            path: path.into(),
            framework: "actix".into(),
            role: "server".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 1,
            handler_qualified_name: None,
        }
    }
    fn client(method: &str, path: &str) -> RouteRecord {
        RouteRecord {
            method: method.into(),
            path: path.into(),
            framework: "fetch".into(),
            role: "client".into(),
            file_path: "src/b/consumer.ts".into(),
            start_line: 1,
            end_line: 1,
            handler_qualified_name: None,
        }
    }

    #[test]
    fn exact_path_match_is_one() {
        let s = vec![server("GET", "/users")];
        let c = vec![client("GET", "/users")];
        let out = cross_service_candidates(
            CrossServiceInput { project: "a", routes: &s },
            CrossServiceInput { project: "b", routes: &c },
            0.5,
        );
        assert_eq!(out.len(), 1);
        assert!((out[0].confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn path_with_template_is_eighty_five() {
        let s = vec![server("GET", "/users/:id")];
        let c = vec![client("GET", "/users/42")];
        let out = cross_service_candidates(
            CrossServiceInput { project: "a", routes: &s },
            CrossServiceInput { project: "b", routes: &c },
            0.5,
        );
        assert_eq!(out.len(), 1);
        assert!((out[0].confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn no_match_when_methods_differ_but_paths_align() {
        let s = vec![server("GET", "/users")];
        let c = vec![client("POST", "/users")];
        let out = cross_service_candidates(
            CrossServiceInput { project: "a", routes: &s },
            CrossServiceInput { project: "b", routes: &c },
            0.5,
        );
        assert_eq!(out.len(), 1);
        assert!((out[0].confidence - 0.50).abs() < 1e-6);
    }
}