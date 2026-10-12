//! SCIP support (design §5.1–§5.3): detecting indexer jobs, running them in containers, and
//! decoding `index.scip` files into ranges the rest of the graph understands.

pub mod decode;

pub use decode::{Document, Occurrence, Rejection, Relationship, ScipIndex, SymbolInfo, decode};
pub mod jobs;

pub use jobs::{
    EncodingInputs, Indexer, Job, TsScopes, effective_encoding, jobs, maven_source_encoding,
};
pub mod runner;

pub use runner::{
    ContainerRun, DockerRunner, Network, Outcome, ProxySpec, Runner, docker_available,
};
pub mod egress;

pub use egress::Egress;
pub mod hash;

pub use hash::project_input_hash;
pub mod orchestrate;

pub use orchestrate::{
    Budget, Images, Phase, RECIPE_JAVA, RECIPE_TS, RunStatus, ScipRun, image_for, recipe, run_job,
};
pub mod symbol;

pub use symbol::{Package, last_descriptor_name, may_name, package, type_symbol_of};
