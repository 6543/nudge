//! UI backend abstraction for showing the red alert overlay.
//!
//! The timer core is UI-agnostic. Concrete backends live in submodules:
//!   - [`iced`]: Wayland layer-shell via iced + iced_layershell (default)
//!
//! Future backends (X11, macOS) need only implement [`UiBackend`].

use std::time::Duration;

pub mod iced;

/// Errors a UI backend may return when showing an alert.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("failed to initialise overlay: {0}")]
    Init(String),

    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

/// A UI backend capable of showing a single fullscreen red alert overlay.
///
/// Implementations must:
///   - cover all monitors
///   - render above all windows including fullscreen
///   - NOT grab keyboard or pointer input (visual only)
///   - block until the overlay has been visible for `duration`, then return
pub trait UiBackend {
    /// Show the alert with the given message for the given duration.
    /// Blocks until the overlay has been dismissed.
    fn alert(&self, message: &str, duration: Duration) -> Result<(), UiError>;
}
