//! Semantic similarity configuration and signature types.
//!
//! The canonical public definitions live in `crate::types` with the rest of
//! the graph API. Re-exporting them here keeps the semantic facade cohesive
//! without maintaining a second, incompatible representation.

pub use crate::types::{SemanticConfig, SemanticSignature};
