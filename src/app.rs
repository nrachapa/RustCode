//! Application state and message handling.
//!
//! One `App` per window. The window/store split from the architecture plan
//! lands with Stage 3 (multiple windows).

use iced::widget::{operation, text_editor};
use iced::{Color, Element, Fill, Font, Task, Theme};
use rustcode::document::{Document, save};
use rustcode::workspace::Workspace;
use std::path::PathBuf;

const EDITOR_ID: &str = "editor";

/// Everything one window knows.
pub struct App {
    doc: Option<Document>,
    workspace: Option<Workspace>,
    pub show_explorer: bool,
    /// The name-entry prompt in the explorer, if open.
    prompt: Option<Prompt>,
    status: Option<String>,
    /// Flipped on buffer swaps (undo/redo) to force a full re-highlight:
    /// the widget re-highlights everything when the format *function*
    /// changes, even if the settings are identical.
    hl_gen: bool,
}

/// What the explorer's name prompt is collecting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewFile,
    NewFolder,
    Rename,
}

struct Prompt {
    kind: PromptKind,
    /// The directory the new name lands in (also the rename's parent).
    parent: PathBuf,
    name: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewFile,
    OpenDialog,
    OpenFolder,
    Save,
    SaveAs,
    ToggleExplorer,
    Action(text_editor::Action),
    Indent,
    Undo,
    Redo,
    NodeClick(PathBuf),
    ExploreAction(ExploreKind),
    PromptName(String),
    PromptSubmit,
    PromptCancel,
}

/// Explorer actions; file-targeted ones need a selected node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreKind {
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Refresh,
}

impl From<ExploreKind> for PromptKind {
    fn from(k: ExploreKind) -> Self {
        match k {
            ExploreKind::NewFile => PromptKind::NewFile,
            ExploreKind::NewFolder => PromptKind::NewFolder,
            ExploreKind::Rename => PromptKind::Rename,
            _ => PromptKind::NewFile,
        }
    }
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self {
                doc: Some(Document::untitled()),
                workspace: None,
                show_explorer: false,
                prompt: None,
                status: None,
                hl_gen: false,
            },
            operation::focus(EDITOR_ID),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Action(action) => {
                if let Some(doc) = &mut self.doc {
                    doc.edit(action);
                }
            }
            Message::NewFile => {
                if !self.is_dirty() || confirm_discard() {
                    self.doc = Some(Document::untitled());
                    self.status = None;
                }
            }
            Message::OpenDialog => {
                if let Some(path) = rfd::FileDialog::new().set_title("Open file").pick_file() {
                    self.open_document(&path);
                }
            }
            Message::OpenFolder => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Open folder")
                    .pick_folder()
                {
                    match Workspace::open(&path) {
                        Ok(ws) => {
                            self.workspace = Some(ws);
                            self.show_explorer = true;
                            self.status = None;
                        }
                        Err(e) => self.status = Some(format!("Cannot open folder: {e}")),
                    }
                }
            }
            Message::Save => {
                if self.doc.as_ref().is_some_and(|d| d.path.is_none()) {
                    self.save_as();
                } else {
                    self.save_current();
                }
            }
            Message::SaveAs => self.save_as(),
            Message::ToggleExplorer => self.show_explorer = !self.show_explorer,
            Message::Indent => {
                // iced's Indent edit uses an unconfigurable 8-space width,
                // so Tab pastes four spaces explicitly.
                if let Some(doc) = &mut self.doc {
                    doc.edit(text_editor::Action::Edit(text_editor::Edit::Paste(
                        std::sync::Arc::new("    ".to_string()),
                    )));
                }
            }
            Message::Undo => {
                if let Some(doc) = &mut self.doc
                    && doc.undo()
                {
                    self.hl_gen = !self.hl_gen;
                }
            }
            Message::Redo => {
                if let Some(doc) = &mut self.doc
                    && doc.redo()
                {
                    self.hl_gen = !self.hl_gen;
                }
            }
            Message::NodeClick(path) => {
                if let Some(ws) = self.workspace.as_mut() {
                    ws.selected = Some(path.clone());
                    self.prompt = None;
                    if let Some(node) = ws.find_deep_mut(&path) {
                        let is_dir = node.is_dir;
                        if is_dir {
                            node.expanded = !node.expanded;
                            ws.expand(&path);
                        } else {
                            self.open_document(&path);
                        }
                    }
                }
            }
            Message::ExploreAction(kind) => self.explore_action(kind),
            Message::PromptName(name) => {
                if let Some(prompt) = &mut self.prompt {
                    prompt.name = name;
                }
            }
            Message::PromptSubmit => self.submit_prompt(),
            Message::PromptCancel => self.prompt = None,
        }
        Task::none()
    }

    /// Opens `path` as the current document, guarding unsaved changes.
    fn open_document(&mut self, path: &std::path::Path) {
        match Document::open(path) {
            Ok(doc) => {
                if !self.is_dirty() || confirm_discard() {
                    self.doc = Some(doc);
                    self.status = None;
                }
            }
            Err(e) => {
                self.status = Some(format!("Cannot open {}: {e}", path.display()));
            }
        }
    }

    fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty)
    }

    fn save_current(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            return;
        };
        match save::atomic(&path, &doc.text_for_saving()) {
            Ok(()) => {
                doc.dirty = false;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("Save failed: {e}")),
        }
    }

    fn save_as(&mut self) {
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let filename = doc.title();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save file as")
            .set_file_name(&filename)
            .save_file()
        {
            self.doc.as_mut().unwrap().set_path(path.clone());
            match save::atomic(&path, &self.doc.as_ref().unwrap().text_for_saving()) {
                Ok(()) => {
                    self.doc.as_mut().unwrap().dirty = false;
                    self.status = None;
                }
                Err(e) => self.status = Some(format!("Save failed: {e}")),
            }
            // A brand-new file may have appeared in the explorer.
            if let (Some(ws), Some(parent)) = (self.workspace.as_mut(), path.parent())
                && parent == ws.root.as_path()
            {
                ws.relist(parent);
            }
        }
    }

    fn explore_action(&mut self, kind: ExploreKind) {
        let Some(ws) = self.workspace.as_ref() else {
            return;
        };
        let selected = match &ws.selected {
            Some(s) => s.clone(),
            None => return,
        };
        let parent = selected
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| ws.root.clone());

        match kind {
            ExploreKind::Refresh => {
                if let Some(ws) = self.workspace.as_mut() {
                    ws.refresh();
                }
            }
            ExploreKind::Delete => {
                if !selected.is_file() {
                    self.status =
                        Some("Delete works on files only (directories are not deleted).".into());
                    return;
                }
                let yes = rfd::MessageDialog::new()
                    .set_title("Delete file")
                    .set_description(format!(
                        "Delete {}? This cannot be undone.",
                        selected.display()
                    ))
                    .set_buttons(rfd::MessageButtons::OkCancel)
                    .show();
                if yes != rfd::MessageDialogResult::Ok {
                    return;
                }
                match Workspace::delete_file(&selected) {
                    Ok(()) => {
                        if let Some(ws) = self.workspace.as_mut() {
                            ws.relist(&parent);
                            ws.selected = None;
                        }
                        self.status = None;
                    }
                    Err(e) => self.status = Some(format!("Delete failed: {e}")),
                }
            }
            ExploreKind::Rename => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Rename,
                    parent,
                    name: selected
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                });
            }
            ExploreKind::NewFile | ExploreKind::NewFolder => {
                // Next to the selection: into the selected folder if it is
                // one, else into the selected file's directory.
                let dir = if selected.is_dir() { selected } else { parent };
                self.prompt = Some(Prompt {
                    kind: kind.into(),
                    parent: dir,
                    name: String::new(),
                });
            }
        }
    }

    fn submit_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let name = prompt.name.trim();
        if name.is_empty() {
            self.status = Some("Name cannot be empty.".into());
            return;
        }
        let (dir, result) = match prompt.kind {
            PromptKind::Rename => {
                let target = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.selected.as_ref())
                    .map(|s| s.to_path_buf())
                    .unwrap_or_default();
                (
                    target
                        .parent()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| prompt.parent.clone()),
                    Workspace::rename(&target, name),
                )
            }
            PromptKind::NewFile => (
                prompt.parent.clone(),
                Workspace::create(&prompt.parent, name, false),
            ),
            PromptKind::NewFolder => (
                prompt.parent.clone(),
                Workspace::create(&prompt.parent, name, true),
            ),
        };
        match result {
            Ok(_) => {
                if let Some(ws) = self.workspace.as_mut() {
                    ws.relist(&dir);
                    if let Some(node) = ws.find_deep_mut(&dir)
                        && node.is_dir
                    {
                        node.expanded = true;
                        ws.expand(&dir);
                    }
                }
                self.status = None;
            }
            Err(e) => self.status = Some(format!("Operation failed: {e}")),
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn view(&self) -> Element<'_, Message> {
        let editor: Element<'_, Message> = self.editor_view();

        let mut body = iced::widget::Row::new();
        body = body.spacing(0);
        if self.show_explorer {
            let prompt_name = self.prompt.as_ref().map(|p| p.name.as_str());
            body = body.push(crate::explorer::view(&self.workspace, prompt_name));
        }
        let right = iced::widget::column!(editor, self.status_view()).spacing(0);
        body = body.push(right);

        iced::widget::column![crate::toolbar::view(self), body]
            .spacing(0)
            .height(Fill)
            .padding(0)
            .into()
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let Some(doc) = self.doc.as_ref() else {
            return iced::widget::center(iced::widget::text("Open a file to get started")).into();
        };

        let editor = text_editor(&doc.content)
            .id(EDITOR_ID)
            .font(Font::with_name("Menlo"))
            .size(13)
            .padding(10)
            .height(Fill)
            .wrapping(iced::widget::text::Wrapping::None)
            .on_action(Message::Action)
            .highlight_with::<iced::highlighter::Highlighter>(
                iced::highlighter::Settings {
                    theme: iced::highlighter::Theme::Base16Ocean,
                    token: doc.language.token().to_string(),
                },
                if self.hl_gen {
                    highlight_format_b
                } else {
                    highlight_format_a
                },
            )
            .key_binding(|key_press: text_editor::KeyPress| {
                use iced::keyboard::{Key, key};
                let cmd = key_press.modifiers.command() || key_press.modifiers.control();
                let custom: Option<Message> = match key_press.key.to_latin(key_press.physical_key) {
                    Some('z') if cmd && key_press.modifiers.shift() => Some(Message::Redo),
                    Some('z') if cmd => Some(Message::Undo),
                    Some('y') if cmd => Some(Message::Redo),
                    Some('s') if cmd => Some(Message::Save),
                    Some('o') if cmd => Some(Message::OpenDialog),
                    _ => None,
                };
                if let Some(msg) = custom {
                    Some(text_editor::Binding::Custom(msg))
                } else {
                    // iced ships no Tab→indent binding, so we provide one.
                    match key_press.modified_key.as_ref() {
                        Key::Named(key::Named::Tab) => {
                            Some(text_editor::Binding::Custom(Message::Indent))
                        }
                        _ => text_editor::Binding::from_key_press(key_press),
                    }
                }
            });

        editor.into()
    }

    fn status_view(&self) -> Element<'_, Message> {
        let doc = self.doc();
        let mut left = String::new();
        if let Some(doc) = doc {
            let cursor = doc.content.cursor();
            left = format!(
                "{}{}{}  Ln {}, Col {}",
                doc.title(),
                if doc.dirty { " •" } else { "" },
                " ",
                cursor.position.line + 1,
                cursor.position.column + 1
            );
        }
        let faded = Color::from([0.6, 0.6, 0.65]);
        let content = if let Some(status) = &self.status {
            iced::widget::row![
                iced::widget::text(left).size(11).color(faded),
                iced::widget::space::horizontal(),
                iced::widget::text(status)
                    .size(11)
                    .color(Color::from([1.0, 0.45, 0.45])),
            ]
            .padding([4, 10])
        } else {
            iced::widget::row![
                iced::widget::text(left).size(11).color(faded),
                iced::widget::space::horizontal(),
            ]
            .padding([4, 10])
        };
        iced::widget::container(content).width(Fill).into()
    }

    /// The current document (also used by the toolbar).
    pub fn doc(&self) -> Option<&Document> {
        self.doc.as_ref()
    }
}

/// Two *identical* highlight format functions. Passing a different fn
/// pointer makes the widget think the highlighter changed and re-highlight
/// every line — needed after undo/redo replaces the buffer wholesale.
fn highlight_format_a(
    highlight: &iced::highlighter::Highlight,
    _theme: &Theme,
) -> iced_core::text::highlighter::Format<iced_core::Font> {
    highlight.to_format()
}

fn highlight_format_b(
    highlight: &iced::highlighter::Highlight,
    _theme: &Theme,
) -> iced_core::text::highlighter::Format<iced_core::Font> {
    highlight.to_format()
}

/// Native yes/no; returns `true` to proceed (i.e. the user chose Yes).
fn confirm_discard() -> bool {
    let answer = rfd::MessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("The current file has unsaved changes.\nDiscard them and continue?")
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    answer == rfd::MessageDialogResult::Yes
}
