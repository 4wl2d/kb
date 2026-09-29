//! Canonical data model: records, registries, trusted configuration, routing fixtures.

pub mod ids;
pub mod profile;
pub mod record;
pub mod registry;
pub mod routing;

pub use profile::*;
pub use record::*;
pub use registry::{Registry, RegistryData};
