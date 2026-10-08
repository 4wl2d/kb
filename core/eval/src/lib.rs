//! Optional replay tooling. None of this crate is linked into the knowledge engine.
pub mod accounting;
pub mod adapters;
pub mod files;
pub mod isolation;
pub mod model;
pub mod replay;
pub mod statistics;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn ensure(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}
