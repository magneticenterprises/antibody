//! Administration and inspection endpoints used by the dashboard.

mod access;
mod activity;
mod jobs;
mod settings;
mod workflow;

pub use access::*;
pub use activity::*;
pub use jobs::*;
pub use settings::*;
pub use workflow::*;
