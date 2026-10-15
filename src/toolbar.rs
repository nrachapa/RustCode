//! The toolbar: mouse-first access to every essential file operation.

use crate::app::{App, Message};
use iced::Element;
use iced::widget::{button, row, space, text};

/// Open / New / Save / Save As, plus a live filename + language readout.
pub fn view(app: &App) -> Element<'_, Message> {
    let doc = app.doc();
    let label = match doc {
        Some(doc) => {
            let mut label = doc.title();
            if doc.dirty {
                label.push_str(" •");
            }
            label.push_str("  ");
            label.push_str(doc.language.label());
            label
        }
        None => String::from("no file"),
    };

    row![
        button("New").on_press(Message::NewFile),
        button("Open…").on_press(Message::OpenDialog),
        button("Open Folder…").on_press(Message::OpenFolder),
        button(if app.show_explorer {
            "Hide Explorer"
        } else {
            "Show Explorer"
        })
        .on_press(Message::ToggleExplorer),
        button("Save").on_press(Message::Save),
        button("Save As…").on_press(Message::SaveAs),
        button("Undo").on_press(Message::Undo),
        button("Redo").on_press(Message::Redo),
        space::horizontal(),
        text(label).size(12),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .height(36)
    .padding([10.0, 8.0])
    .into()
}
