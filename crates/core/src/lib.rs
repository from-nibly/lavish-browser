//! GUI-free browser domain model and reducers.

pub mod labels;
pub mod model;

pub use lavish_browser_protocol::ProjectKey;
pub use model::{BrowserModel, Document, DocumentKey, DocumentLifecycle, OpenOutcome, Project};

/// Persistence implementations consume and restore the pure model through this seam.
pub trait BrowserStateStore {
    type Error;

    fn save(&self, model: &BrowserModel) -> Result<(), Self::Error>;
    fn load(&self) -> Result<BrowserModel, Self::Error>;
}
