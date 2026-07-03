//! Error type for process simulation.

use thiserror::Error;

/// Errors produced while simulating a process recipe.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// The underlying GDS library failed to flatten.
    #[error("gds error: {0}")]
    Gds(#[from] vcad_gdsii::GdsError),
    /// A recipe step references a GDS layer with no geometry anywhere in
    /// the layout — almost always a typo in the recipe.
    #[error("recipe references GDS layer {0}, which has no geometry in the layout")]
    UnknownMaskLayer(i16),
    /// A recipe parameter is out of range (non-positive thickness, empty
    /// span, NaN, …).
    #[error("bad recipe: {0}")]
    BadRecipe(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, ProcessError>;
