//! The `Meta` transform seam: rendering an endpoint's [`Description`] into a
//! requested representation type.
//!
//! Defined in core so the kernel can route `Meta` uniformly — but with **no RDF
//! dependency**. `ikigai-vocab` provides an implementation (Turtle, text, …) and
//! the host injects it into the kernel via [`Kernel::with_meta_renderer`]. This
//! is the first piece of the transform layer: an endpoint returns its canonical
//! [`Description`], and the *requested* type drives the projection.
//!
//! [`Description`]: crate::Description
//! [`Kernel::with_meta_renderer`]: crate::Kernel::with_meta_renderer

use crate::describe::Description;
use crate::error::Result;
use crate::repr::{ReprType, Representation};

/// Renders an endpoint's self-description into a target representation type
/// (e.g. `text/turtle`, `text/plain`, `text/html`).
pub trait MetaRenderer: Send + Sync {
    /// Render `description` as `target`.
    fn render(&self, description: &Description, target: &ReprType) -> Result<Representation>;
}
