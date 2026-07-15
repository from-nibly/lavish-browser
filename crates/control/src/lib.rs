//! Native browser-control transport seam.

use lavish_browser_protocol::{RequestEnvelope, ResponseEnvelope};

/// A synchronous request boundary shared by native callers.
pub trait BrowserControl {
    type Error;

    fn request(&self, request: &RequestEnvelope) -> Result<ResponseEnvelope, Self::Error>;
}
