//! SCIP support (design §5.1–§5.3): detecting indexer jobs, running them in containers, and
//! decoding `index.scip` files into ranges the rest of the graph understands.

pub mod decode;

pub use decode::{Document, Occurrence, Relationship, ScipIndex, SymbolInfo, decode};
pub mod jobs;

pub use jobs::{Indexer, Job, jobs};
pub mod runner;

pub use runner::{ContainerRun, DockerRunner, Network, Outcome, Runner, docker_available};
pub mod hash;

pub use hash::project_input_hash;
pub mod orchestrate;

pub use orchestrate::{Budget, RunStatus, ScipRun, image_for, resolve_image, run_job};
pub mod symbol;

pub use symbol::{Package, last_descriptor_name, package};
