//! Native in-process Dockerfile builder and local image registry.
//!
//! Allows constructing OCI container rootfs images directly from Dockerfiles
//! without requiring an external container runtime daemon.

pub mod dockerignore;
pub mod executor;
pub mod parser;

pub use dockerignore::Dockerignore;
pub use executor::{
    try_fetch_local_tag, BuildEngine, BuildOptions, BuildResult, LocalImageRecord,
    LocalImageRegistry,
};
pub use parser::{CommandForm, Dockerfile, Instruction};
