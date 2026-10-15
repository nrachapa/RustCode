mod app;
mod explorer;
mod toolbar;

use app::App;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .theme(App::theme)
        .default_font(iced::Font::MONOSPACE)
        .run()
}
