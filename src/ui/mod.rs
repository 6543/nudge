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
    /// Show the alert for `duration`, then return.
    ///
    /// `message` is the primary text (large, centered).
    /// `subtitle` is optional smaller text shown below the message (e.g.
    /// "next nudge in 2m 30s" or "locking now"). Backends should render it
    /// at a noticeably smaller size than `message`, or omit it if `None`.
    fn alert(
        &self,
        message: &str,
        subtitle: Option<&str>,
        duration: Duration,
    ) -> Result<(), UiError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// In-memory test backend. Records every alert call so tests can assert
    /// on the sequence the timer loop produced.
    struct MockUi {
        calls: RefCell<Vec<(String, Option<String>, Duration)>>,
    }

    impl MockUi {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl UiBackend for MockUi {
        fn alert(
            &self,
            message: &str,
            subtitle: Option<&str>,
            duration: Duration,
        ) -> Result<(), UiError> {
            self.calls.borrow_mut().push((
                message.to_owned(),
                subtitle.map(str::to_owned),
                duration,
            ));
            Ok(())
        }
    }

    #[test]
    fn mock_backend_records_calls() {
        // The trait is implementable by a non-iced type, and the recorded
        // arguments match what was passed in. This guards the trait
        // contract: no hidden assumptions, no required state on the
        // implementor, no panics on call.
        let ui = MockUi::new();
        ui.alert("hi", Some("sub"), Duration::from_secs(1)).unwrap();
        ui.alert("bye", None, Duration::from_secs(2)).unwrap();
        let calls = ui.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "hi");
        assert_eq!(calls[0].1.as_deref(), Some("sub"));
        assert_eq!(calls[0].2, Duration::from_secs(1));
        assert_eq!(calls[1].0, "bye");
        assert_eq!(calls[1].1, None);
        assert_eq!(calls[1].2, Duration::from_secs(2));
    }

    #[test]
    fn ui_error_messages_are_reasonable() {
        // Errors should be informative when displayed.
        let init = UiError::Init("display unavailable".into());
        let runtime = UiError::Runtime("surface lost".into());
        assert!(init.to_string().contains("display unavailable"));
        assert!(runtime.to_string().contains("surface lost"));
    }
}
