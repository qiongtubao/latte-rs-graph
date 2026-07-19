//! Phase 5 — Cypher v1 subset (parser + planner + executor).
//!
//! Public surface:
//! - [`parse_cypher`] turns a string into a [`crate::query::cypher::ast::CypherQuery`].
//! - [`execute_cypher`] runs the parsed query against a [`crate::storage::SqliteStorage`]
//!   and yields `Vec<CypherRow>` ready for the engine to flatten to JSON.
//!
//! Anything outside the subset documented in
//! `src/query/cypher/parser.rs` is rejected with a precise message —
//! the parser never silently drops a clause.

pub mod ast;
pub mod executor;
pub mod lexer;
pub mod parser;
pub mod planner;

pub use ast::*;
pub use executor::{execute_cypher, CypherRow, CypherValue};
pub use parser::parse_cypher;
