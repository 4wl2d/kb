//! `kb`: typed, verifiable engineering knowledge base engine.
//!
//! Layering: pure logic (`model`, `parse`, `validate`, `normalize`, `glob`, `context`) is
//! separate from adapters (`git`, `host`, `snapshot`, `overlay`, `source`, `index`,
//! `integrate`, `update`) and from the CLI (`cli`, `output`).

pub mod cli;
pub mod code;
pub mod context;
pub mod corpus;
pub mod coverage;
pub mod diag;
pub mod doctor;
pub mod env;
pub mod error;
pub mod evaluation;
pub mod freshness;
pub mod git;
pub mod glob;
pub mod host;
pub mod impact;
pub mod index;
pub mod init;
pub mod integrate;
pub mod knowledge;
pub mod migrate;
pub mod model;
pub mod normalize;
pub mod output;
pub mod overlay;
pub mod parse;
pub mod process;
pub mod propose;
pub mod provenance;
pub mod receipts;
pub mod schema_export;
pub mod scope;
pub mod snapshot;
pub mod source;
pub mod trust;
pub mod update;
pub mod usage;
pub mod util;
pub mod validate;
pub mod verify;
pub mod versions;
