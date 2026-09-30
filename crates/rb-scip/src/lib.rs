//! SCIP support (design §5.1–§5.3): decoding `index.scip` files into ranges the rest of the graph
//! understands, and (later) running indexers in containers.

pub mod decode;

pub use decode::{Document, Occurrence, Relationship, ScipIndex, SymbolInfo, decode};
pub mod jobs;

pub use jobs::{Indexer, Job, jobs};
