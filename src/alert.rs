//! Minimal iced application that displays a fullscreen red overlay with
//! centered text, then exits after a fixed duration.
//!
//! Lifecycle: spawned per alert. Runs `Application::run`, blocks until the
//! internal countdown subscription fires `Tick(Done)` which dispatches an
//! `Action::Exit`. Returns control to the caller.
//!
//! The overlay is configured to NOT grab keyboard or pointer input — see
//! `KeyboardInteractivity::None`. This is intentional: the alert is a visual
//! nudge, not coercion.

use std::time::{Duration, Instant};

use iced::{Color, Element, Length, Task, Theme};
use iced::widget::{container, text};
use iced_layershell::Application;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::to_layer_message;

#[derive(Debug, Clone)]
pub struct Flags {
    pub message: String,
    pub duration: Duration,
}

pub struct AlertApp {
    message: String,
    deadline: Instant,
}

#[to_layer_message]
#[derive(Debug, Clone)]
pub enum Message {
    Tick,
}

impl Application for AlertApp {
    type Executor = iced::executor::Default;
    type Flags = Flags;
    type Message = Message;
    type Theme = Theme;

    fn new(flags: Flags) -> (Self, Task<Message>) {
        let app = Self {
            message: flags.message,
            deadline: Instant::now() + flags.duration,
        };
        (app, Task::none())
    }

    fn namespace(&self) -> String {
        "nudge.alert".into()
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Tick at 60Hz; we only need to know "is the deadline past?". Cheap
        // and avoids a second timer/clock crate.
        iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tick => {
                if Instant::now() >= self.deadline {
                    // Subscription fired past the deadline -> exit the app.
                    iced_runtime::task::effect(iced_runtime::Action::Exit)
                } else {
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message, Theme> {
        // Solid red background, centered white message text, large.
        let label = text(&self.message)
            .size(96)
            .color(Color::WHITE);

        container(label)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(1.0, 0.0, 0.0))),
                ..container::Style::default()
            })
            .into()
    }
}

/// Build the layer-shell settings for the alert overlay.
///
/// Anchor on all four edges with no size override -> cover the entire output.
/// Layer = Overlay -> sits above fullscreen windows.
/// KeyboardInteractivity::None -> no input grab, purely visual.
pub fn settings(flags: Flags) -> Settings<Flags> {
    Settings {
        layer_settings: LayerShellSettings {
            size: None,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::None,
            margin: (0, 0, 0, 0),
            exclusive_zone: -1,
            ..Default::default()
        },
        flags,
        antialiasing: true,
        id: Some("nudge".into()),
        ..Default::default()
    }
}
