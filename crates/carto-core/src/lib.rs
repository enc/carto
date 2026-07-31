//! carto-core: repo traversal, graph store, and the invariant substrate
//! (taint, pathguard) that everything else in the workspace compiles
//! against. No I/O besides fs; no clap, no rmcp (spec §3.1).

pub mod consts;
pub mod error;
pub mod gitinfo;
pub mod graph;
pub mod lang;
pub mod outdir;
pub mod pathguard;
pub mod query;
pub mod redact;
pub mod taint;
pub mod walk;

pub use error::{Error, ErrorKind, Result};
