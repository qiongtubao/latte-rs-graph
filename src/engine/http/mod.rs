//! HTTP route detection + cross-service route/call-site matching (Phase 11).
//!
//! Detects HTTP route definitions (server-side) and outbound HTTP call sites
//! (client-side) across 4 languages × several frameworks each. The matchers
//! live in [`patterns`]; the AST walker + dedupe lives in [`route_extract`];
//! the cross-service scorer lives in [`cross_service`].
//!
//! Out of scope for v1: gRPC, GraphQL, tRPC, and pub/sub channels. Only HTTP.
pub mod cross_service;
pub mod patterns;
pub mod route_extract;

// Re-exports for the convenience of `crate::engine::*` consumers.
pub use crate::types::{normalize_http_path, path_segments, CrossServiceCandidate};
pub use cross_service::{cross_service_candidates, CrossServiceInput};
pub use route_extract::{
    extract_routes_and_calls, CallSite, ExtractResult, RouteDefinition,
};