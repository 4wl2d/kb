//! Canonical data model: records, registries, trusted configuration, routing fixtures.

pub mod code;
pub mod evolution;
pub mod ids;
pub mod profile;
pub mod record;
pub mod registry;
pub mod routing;

pub use code::*;
pub use evolution::*;
pub use profile::*;
pub use record::*;
pub use registry::{Registry, RegistryData};
