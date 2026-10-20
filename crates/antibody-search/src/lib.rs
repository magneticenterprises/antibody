//! Authorized retrieval: exact signatures/symbols/paths, PostgreSQL full-text
//! search and optional pgvector similarity, reranked for applicability,
//! evidence quality, recency and source availability.
//!
//! Every query is restricted to the caller's `AccessScope` *before* any
//! candidate is scored, so unauthorized repositories cannot influence
//! results, counts or latency-visible work.

mod engine;
mod views;

pub use engine::{SearchConfig, SearchEngine, SearchError};
pub use views::verification_summary;
