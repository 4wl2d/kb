//! The `kb` command as the engine selector (docs/architecture.md, section 9; ADR 0017).
//!
//! One installed `kb` serves every KB checkout on a machine while each checkout keeps the
//! engine it pins: [`select`] runs the engine built from the checkout's own engine sources
//! (identified by their fingerprint), in-process when that is this binary. `kb engine ...`
//! manages the runtimes in the per-user store; `kb self update` replaces the installed `kb`.

pub mod archive;
pub mod cargo;
pub mod engine;
pub mod fetch;
pub mod fingerprint;
pub mod platform;
pub mod resolve;
pub mod select;
pub mod selfupdate;
pub mod stamp;
pub mod store;

pub use select::{Selected, select};
