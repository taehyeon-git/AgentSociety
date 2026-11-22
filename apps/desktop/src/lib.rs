//! Native dashboard and its single-writer, bounded-publication worker.

mod dashboard;
mod files;
pub mod worker;

pub use dashboard::Dashboard;
