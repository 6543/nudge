//! Iced + iced_layershell implementation of [`UiBackend`].
//!
//! This file is a placeholder; the real implementation lands in a follow-up
//! commit together with the AlertApp.

use std::time::Duration;

use super::{UiBackend, UiError};

/// Wayland layer-shell UI backend. Renders a fullscreen red overlay via
/// `iced_layershell` on every output, above all windows including fullscreen.
pub struct IcedLayerShellUi;

impl IcedLayerShellUi {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IcedLayerShellUi {
    fn default() -> Self {
        Self::new()
    }
}

impl UiBackend for IcedLayerShellUi {
    fn alert(&self, _message: &str, _duration: Duration) -> Result<(), UiError> {
        Err(UiError::Init("iced backend not yet implemented".into()))
    }
}
