//! The file-explorer sidebar: tree, actions, and the name prompt.

use crate::app::Message;
use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, text, text_input,
};
use iced::{Alignment, Background, Color, Element, Fill, Length};
use rustcode::workspace::{Node, Workspace};
use std::path::PathBuf;

const PANEL_WIDTH: f32 = 240.0;
const SIDE_BG: [f32; 3] = [0.11, 0.11, 0.13];
const SELECTED_BG: [f32; 3] = [0.2, 0.27, 0.36];
const FADED: [f32; 3] = [0.5, 0.5, 0.55];

/// The whole sidebar.
pub fn view<'a>(
    workspace: &'a Option<Workspace>,
    prompt_name: Option<&'a str>,
) -> Element<'a, Message> {
    let Some(ws) = workspace else {
        let placeholder = container(
            column![
                text("No folder open").size(13),
                text("Use Open Folder in the toolbar")
                    .size(11)
                    .color(Color::from(FADED)),
            ]
            .align_x(Alignment::Center)
            .spacing(4)
            .padding([12.0, 12.0]),
        )
        .width(Length::Fixed(PANEL_WIDTH))
        .height(Fill)
        .style(|_| side_bg());
        return placeholder.into();
    };

    let selected = ws.selected.as_ref();
    let has_selection = selected.is_some();

    let actions = row![
        button("New File").on_press_maybe(
            has_selection.then_some(Message::ExploreAction(crate::app::ExploreKind::NewFile))
        ),
        button("New Folder").on_press_maybe(
            has_selection.then_some(Message::ExploreAction(crate::app::ExploreKind::NewFolder))
        ),
        button("Rename").on_press_maybe(
            has_selection.then_some(Message::ExploreAction(crate::app::ExploreKind::Rename))
        ),
        button("Delete").on_press_maybe(
            has_selection.then_some(Message::ExploreAction(crate::app::ExploreKind::Delete))
        ),
        button("Refresh").on_press(Message::ExploreAction(crate::app::ExploreKind::Refresh)),
    ]
    .spacing(4)
    .padding([8.0, 8.0]);

    let prompt: Element<'a, Message> = match prompt_name {
        Some(name) => {
            let entry = text_input("name", name)
                .on_input(Message::PromptName)
                .on_submit(Message::PromptSubmit)
                .size(12)
                .padding(4);
            let go = button("Go").on_press(Message::PromptSubmit);
            let cancel = button("Cancel").on_press(Message::PromptCancel);
            let p = container(
                row![entry, space::horizontal(), go, cancel]
                    .spacing(4)
                    .align_y(Alignment::Center),
            )
            .padding([4.0, 8.0]);
            Element::from(p)
        }
        None => Element::from(space::horizontal()),
    };

    // Build the tree as a flat list of rows (each indented by depth).
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    if ws.nodes.is_empty() {
        let empty = text("(empty)").size(12).color(Color::from(FADED));
        rows.push(container(empty).padding([4.0, 12.0]).into());
    } else {
        for node in &ws.nodes {
            collect(&mut rows, node, 0, selected);
        }
    }

    let mut tree = iced::widget::Column::new();
    for r in rows {
        tree = tree.push(r);
    }
    let tree = scrollable(tree).width(Fill).height(Fill);

    let header = row![
        text(format!("{}  ", ws.title())).size(12),
        space::horizontal(),
        button("…").on_press(Message::OpenFolder).padding(2),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .padding([8.0, 8.0]);

    let panel = column![header, actions, prompt, tree].spacing(0);

    container(panel)
        .width(Length::Fixed(PANEL_WIDTH))
        .height(Fill)
        .style(|_| side_bg())
        .into()
}

fn side_bg() -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from(SIDE_BG))),
        ..container::Style::default()
    }
}

/// Appends a node and (if expanded) its children as flat, depth-indented rows.
fn collect<'a>(
    rows: &mut Vec<Element<'a, Message>>,
    node: &'a Node,
    depth: usize,
    selected: Option<&'a PathBuf>,
) {
    let is_selected = selected.is_some_and(|s| s == &node.path);
    let prefix = if node.is_dir {
        if node.expanded && node.children.is_some() {
            "▾ "
        } else {
            "▸ "
        }
    } else {
        "   "
    };

    let label = if node.unreadable {
        format!("{prefix}{} (unreadable)", node.name)
    } else {
        format!("{prefix}{}", node.name)
    };

    let indent = 8.0 + depth as f32 * 14.0;
    let inner = row![text(label).size(13)]
        .width(Fill)
        .padding([3.0, indent]);

    let content: Element<'a, Message> = if is_selected {
        let c = container(inner).style(|_| container::Style {
            background: Some(Background::Color(Color::from(SELECTED_BG))),
            ..container::Style::default()
        });
        Element::from(c)
    } else {
        Element::from(inner)
    };

    rows.push(
        mouse_area(content)
            .on_press(Message::NodeClick(node.path.clone()))
            .into(),
    );

    if node.is_dir && node.expanded {
        if let Some(children) = &node.children {
            for child in children {
                collect(rows, child, depth + 1, selected);
            }
        }
        if node.unreadable {
            let note = text("(cannot read)").size(11).color(Color::from(FADED));
            rows.push(
                container(note)
                    .padding([2.0, 8.0 + (depth + 1) as f32 * 14.0])
                    .into(),
            );
        }
    }
}
