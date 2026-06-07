//! latte-rs-graph — Universal code graph library with swappable backend engines.
//!
//! # Architecture
//!
//! ```text
//! GraphProvider (trait)
//!     │
//!     ├── TreeSitterEngine (default, Rust native)
//!     ├── CodegraphBridge  (calls npm codegraph)
//!     └── GitNexusBridge   (calls Python GitNexus)
//! ```
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use latte_rs_graph::prelude::*;
//!
//! # async fn example() -> Result<(), latte_rs_graph::error::GraphError> {
//! let storage = SqliteStorage::open("/tmp/my-graph.db".as_ref())?;
//! let engine = TreeSitterEngine::new(storage);
//!
//! let report = engine.build("/path/to/project".as_ref(), &BuildOptions::default()).await?;
//! println!("Built: {} nodes, {} edges", report.nodes_created, report.edges_created);
//!
//! let data = engine.graph_data().await?;
//! println!("Total nodes: {}", data.stats.total_nodes);
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod types;
pub mod traits;
pub mod engine;
pub mod storage;
pub mod query;

/// Convenience re-exports for the most common types.
pub mod prelude {
    pub use crate::engine::TreeSitterEngine;
    pub use crate::error::{GraphError, GraphResult};
    pub use crate::storage::{MemoryStorage, SqliteStorage};
    pub use crate::traits::GraphProvider;
    pub use crate::types::*;
}
