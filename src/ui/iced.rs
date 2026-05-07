//! Iced + iced_layershell implementation of [`UiBackend`].
//!
//! Each call to [`UiBackend::alert`] spins up a fresh [`AlertApp`], which
//! blocks until the alert duration has elapsed and then exits. The iced
//! runtime is therefore not kept alive between alerts — the trait method
//! returns to the timer loop as soon as the overlay closes.

use std::time::Duration;

use iced_layershell::Application;

use super::{UiBackend, UiError};
use crate::alert::{self, AlertApp, Flags};

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
    fn alert(
        &self,
        message: &str,
        subtitle: Option<&str>,
        duration: Duration,
    ) -> Result<(), UiError> {
        let flags = Flags {
            message: message.to_owned(),
            subtitle: subtitle.map(str::to_owned),
            duration,
        };
        AlertApp::run(alert::settings(flags))
            .map_err(|e| UiError::Runtime(e.to_string()))
    }
}
