mod app;
mod explorer;
mod terminal_view;
mod toolbar;

use app::App;

/// Raw keyboard events, so the terminal canvas gets keys even though it is
/// not a iced "focused" text widget. (The app acts on them only while the
/// terminal has focus; the editor keeps its own key bindings.)
fn raw_key(
    event: iced::event::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<app::Message> {
    match event {
        iced::event::Event::Keyboard(keyboard) => Some(app::Message::Key(keyboard)),
        _ => None,
    }
}

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .subscription(|_| iced::event::listen_raw(raw_key))
        .theme(App::theme)
        .default_font(iced::Font::MONOSPACE)
        .run()
}
