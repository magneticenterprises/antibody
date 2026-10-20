//! Git/CI webhook verification and parsing, agent transcript normalization
//! and hook payload helpers. Pure functions; no I/O except where noted.

pub mod ci;
pub mod git;
pub mod github;
pub mod hooks;
pub mod signing;
pub mod transcripts;
