//! Application state and message handling.
//!
//! One `App` per window. The window/store split from the architecture plan
//! lands with Stage 3 (multiple windows).

use iced::widget::{button, operation, scrollable, text, text_editor, text_input};
use iced::{Color, Element, Fill, Font, Length, Task, Theme};
use rustcode::document::save;
use rustcode::tabs::Tabs;
use rustcode::terminal::{self, AnsiFilter, Session};
use rustcode::workspace::Workspace;
use std::path::{Path, PathBuf};
use std::time::Duration;

const EDITOR_ID: &str = "editor";
const TERM_SCROLL_ID: &str = "term-scroll";
/// Quiescence before autosave fires.
const AUTOSAVE_DELAY: Duration = Duration::from_secs(2);

/// Everything one window knows.
pub struct App {
    tabs: Tabs,
    workspace: Option<Workspace>,
    pub show_explorer: bool,
    pub show_terminal: bool,
    /// The name-entry prompt in the explorer, if open.
    prompt: Option<Prompt>,
    status: Option<String>,
    /// Flipped on buffer swaps (undo/redo/tab switch) to force a full
    /// re-highlight: the widget re-highlights everything when the format
    /// *function* pointer changes, even if the settings are identical.
    hl_gen: bool,
    // Terminal: a pty-backed shell, line-oriented.
    term: Option<Session>,
    term_filter: AnsiFilter,
    term_output: String,
    term_input: String,
    term_tx: futures::channel::mpsc::UnboundedSender<TermEvent>,
    // Autosave debounce: a background thread fires a tick after
    // AUTOSAVE_DELAY of no edits.
    autosave_seq: u64,
    autosave_tx: std::sync::mpsc::Sender<u64>,
    // Stream receivers, consumed by the boot task.
    term_rx: Option<futures::channel::mpsc::UnboundedReceiver<TermEvent>>,
    autosave_rx: Option<futures::channel::mpsc::UnboundedReceiver<u64>>,
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

/// Events from the terminal reader thread.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Raw pty bytes (ANSI stripped on arrival).
    Output(Vec<u8>),
    /// The shell process exited.
    Exited,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewFile,
    OpenDialog,
    OpenFolder,
    Save,
    SaveAs,
    ToggleExplorer,
    ToggleTerminal,
    Action(text_editor::Action),
    Indent,
    Undo,
    Redo,
    ActivateTab(usize),
    CloseTab(usize),
    NodeClick(PathBuf),
    ExploreAction(ExploreKind),
    PromptName(String),
    PromptSubmit,
    PromptCancel,
    TermInput(String),
    TermSubmit(String),
    TermEvent(TermEvent),
    AutosaveTick(u64),
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
        let (term_tx, term_rx) = futures::channel::mpsc::unbounded::<TermEvent>();
        let (autosave_tx, autosave_rx) = futures::channel::mpsc::unbounded::<u64>();
        let (deb_tx, deb_rx) = std::sync::mpsc::channel::<u64>();

        let mut app = Self {
            tabs: Tabs::with_untitled(),
            workspace: None,
            show_explorer: false,
            show_terminal: false,
            prompt: None,
            status: None,
            hl_gen: false,
            term: None,
            term_filter: AnsiFilter::default(),
            term_output: String::new(),
            term_input: String::new(),
            term_tx: term_tx.clone(),
            autosave_seq: 0,
            autosave_tx: deb_tx,
            term_rx: Some(term_rx),
            autosave_rx: Some(autosave_rx),
        };

        // The shell starts in $HOME; opening a folder respawns it there.
        let home = dirs_home();
        if let Ok(session) = Session::spawn(&home) {
            app.term_output = format!("$ shell in {}\n", home.display());
            app.term = Some(session);
            app.spawn_term_reader();
        }

        // Autosave debounce: one long-lived thread.
        std::thread::spawn(move || {
            while let Ok(seq) = deb_rx.recv() {
                let mut pending = seq;
                loop {
                    std::thread::sleep(AUTOSAVE_DELAY);
                    // Still editing? Keep waiting.
                    match deb_rx.try_recv() {
                        Ok(s) => pending = s,
                        Err(_) => break,
                    }
                }
                let _ = autosave_tx.unbounded_send(pending);
            }
        });

        let mut tasks = vec![operation::focus(EDITOR_ID)];
        if let Some(rx) = app.term_rx.take() {
            tasks.push(Task::stream(rx).map(Message::TermEvent));
        }
        if let Some(rx) = app.autosave_rx.take() {
            tasks.push(Task::stream(rx).map(Message::AutosaveTick));
        }
        (app, Task::batch(tasks))
    }

    fn spawn_term_reader(&mut self) {
        let Some(session) = self.term.as_ref() else {
            return;
        };
        let Ok(mut reader) = session.reader() else {
            return;
        };
        let tx = self.term_tx.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.unbounded_send(TermEvent::Exited);
                        break;
                    }
                    Ok(n) => {
                        let _ = tx.unbounded_send(TermEvent::Output(buf[..n].to_vec()));
                    }
                    Err(_) => break,
                }
            }
        });
    }

    /// (Re)spawns the shell in `cwd`, resetting the output buffer.
    fn respawn_terminal(&mut self, cwd: &Path) {
        self.term = match Session::spawn(cwd) {
            Ok(session) => {
                self.term_filter.reset();
                self.term_output = format!("$ shell in {}\n", cwd.display());
                Some(session)
            }
            Err(e) => {
                self.status = Some(format!("Terminal: {e}"));
                None
            }
        };
        if self.term.is_some() {
            self.spawn_term_reader();
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Action(action) => {
                let is_edit = matches!(action, text_editor::Action::Edit(_));
                if let Some(doc) = self.tabs.active_mut() {
                    doc.edit(action);
                }
                if is_edit {
                    self.schedule_autosave();
                }
            }
            Message::NewFile => {
                // Tabs never discard: New simply adds a tab.
                self.tabs.new_untitled();
                self.hl_gen = !self.hl_gen;
                self.status = None;
            }
            Message::OpenDialog => {
                if let Some(path) = rfd::FileDialog::new().set_title("Open file").pick_file() {
                    self.open_file(&path);
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
                            // The shell follows the project.
                            if self.term.is_some() || self.show_terminal {
                                self.respawn_terminal(&path);
                            }
                        }
                        Err(e) => self.status = Some(format!("Cannot open folder: {e}")),
                    }
                }
            }
            Message::Save => {
                if self.doc().is_some_and(|d| d.path.is_none()) {
                    self.save_as();
                } else {
                    self.save_active();
                }
            }
            Message::SaveAs => self.save_as(),
            Message::ToggleExplorer => self.show_explorer = !self.show_explorer,
            Message::ToggleTerminal => {
                self.show_terminal = !self.show_terminal;
                if self.show_terminal && self.term.is_none() {
                    self.respawn_terminal(&dirs_home());
                }
            }
            Message::Indent => {
                // iced's Indent edit uses an unconfigurable 8-space width,
                // so Tab pastes four spaces explicitly.
                if let Some(doc) = self.tabs.active_mut() {
                    doc.edit(text_editor::Action::Edit(text_editor::Edit::Paste(
                        std::sync::Arc::new("    ".to_string()),
                    )));
                    self.schedule_autosave();
                }
            }
            Message::Undo => {
                if let Some(doc) = self.tabs.active_mut()
                    && doc.undo()
                {
                    self.hl_gen = !self.hl_gen;
                }
            }
            Message::Redo => {
                if let Some(doc) = self.tabs.active_mut()
                    && doc.redo()
                {
                    self.hl_gen = !self.hl_gen;
                }
            }
            Message::ActivateTab(i) => {
                if self.tabs.active != Some(i) {
                    self.tabs.active = Some(i);
                    self.hl_gen = !self.hl_gen; // new content: force re-highlight
                }
            }
            Message::CloseTab(i) => {
                if self.tabs.docs.get(i).is_some_and(|d| d.dirty) && !confirm_discard() {
                    return Task::none();
                }
                self.tabs.close(i);
                self.hl_gen = !self.hl_gen;
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
                        }
                    }
                }
                // A clicked file becomes (or focuses) a tab.
                if path.is_file() {
                    self.open_file(&path);
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
            Message::TermInput(value) => {
                let old = std::mem::replace(&mut self.term_input, value.clone());
                if let (Some(session), data) = (&mut self.term, term_input_diff(&old, &value))
                    && !data.is_empty()
                {
                    session.write(&data).ok();
                }
            }
            Message::TermSubmit(line) => {
                if let Some(session) = &mut self.term {
                    let mut data = line.clone().into_bytes();
                    data.push(b'\n');
                    session.write(&data).ok();
                }
                self.term_input.clear();
            }
            Message::TermEvent(event) => match event {
                TermEvent::Output(chunk) => {
                    terminal::append_output(&mut self.term_output, &chunk, &mut self.term_filter);
                }
                TermEvent::Exited => {
                    self.term_filter.reset();
                    self.term_output.push_str("(shell exited — restarting)\n");
                    let cwd = self
                        .workspace
                        .as_ref()
                        .map(|w| w.root.clone())
                        .unwrap_or_else(dirs_home);
                    self.respawn_terminal(&cwd);
                }
            },
            Message::AutosaveTick(seq) => {
                if seq == self.autosave_seq {
                    self.autosave_all();
                }
            }
        }
        Task::none()
    }

    /// Bumps the debounce: the background thread (re)arms its timer.
    fn schedule_autosave(&mut self) {
        self.autosave_seq += 1;
        let _ = self.autosave_tx.send(self.autosave_seq);
    }

    /// Autosave: silently saves every open file that is dirty.
    /// Untitled buffers cannot be autosaved (no path yet) — those still need Save As.
    fn autosave_all(&mut self) {
        let mut failed = false;
        for doc in self.tabs.docs.iter_mut() {
            let Some(path) = doc.path.clone() else {
                continue;
            };
            if !doc.dirty {
                continue;
            }
            match save::atomic(&path, &doc.text_for_saving()) {
                Ok(()) => doc.dirty = false,
                Err(e) => {
                    self.status = Some(format!("Autosave failed for {}: {e}", path.display()));
                    failed = true;
                }
            }
        }
        if !failed {
            self.status = None;
        }
    }

    /// Opens `path` as a tab: activates the tab if already open, else adds one.
    fn open_file(&mut self, path: &Path) {
        match self.tabs.open_file(path) {
            Ok(_) => {
                self.hl_gen = !self.hl_gen;
                self.status = None;
            }
            Err(e) => self.status = Some(format!("Cannot open {}: {e}", path.display())),
        }
    }

    fn save_active(&mut self) {
        let Some(doc) = self.tabs.active_mut() else {
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
        let Some(doc) = self.tabs.active() else {
            return;
        };
        let filename = doc.title();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save file as")
            .set_file_name(&filename)
            .save_file()
        {
            let text = if let Some(doc) = self.tabs.active() {
                doc.text_for_saving()
            } else {
                String::new()
            };
            match save::atomic(&path, &text) {
                Ok(()) => {
                    if let Some(doc) = self.tabs.active_mut() {
                        doc.set_path(path.clone());
                        doc.dirty = false;
                    }
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
                        // The file's tab (if any) is now orphaned: close it.
                        if let Some(i) = self
                            .tabs
                            .docs
                            .iter()
                            .position(|d| d.path.as_deref() == Some(selected.as_path()))
                        {
                            self.tabs.close(i);
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

    /// The current document (also used by the toolbar).
    pub fn doc(&self) -> Option<&rustcode::document::Document> {
        self.tabs.active()
    }
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let mut body = iced::widget::Row::new().spacing(0);
        if self.show_explorer {
            let prompt_name = self.prompt.as_ref().map(|p| p.name.as_str());
            body = body.push(crate::explorer::view(&self.workspace, prompt_name));
        }
        let mut center = iced::widget::Column::new().spacing(0);
        center = center.push(self.editor_view());
        if self.show_terminal {
            center = center.push(self.terminal_view());
        }
        center = center.push(self.status_view());
        body = body.push(center);

        iced::widget::column![self.tab_bar(), crate::toolbar::view(self), body]
            .spacing(0)
            .height(Fill)
            .padding(0)
            .into()
    }

    /// The "opened editors" section: one tab per open file.
    fn tab_bar(&self) -> Element<'_, Message> {
        let active = self.tabs.active.unwrap_or(0);
        let faded = Color::from([0.6, 0.6, 0.65]);
        let mut bar = iced::widget::Row::new().spacing(2);
        for (i, doc) in self.tabs.docs.iter().enumerate() {
            let mut label = self.tabs.name(i);
            if doc.dirty {
                label.push_str(" ●");
            }
            let is_active = i == active;
            let name_btn = button(text(label).size(12).color(if is_active {
                Color::from([0.95, 0.95, 0.95])
            } else {
                faded
            }))
            .on_press(Message::ActivateTab(i))
            .padding([5.0, 8.0]);
            let close = button(text("×").size(11).color(faded))
                .on_press(Message::CloseTab(i))
                .padding([3.0, 5.0]);
            let tab = iced::widget::row![name_btn, close].spacing(4);
            bar =
                bar.push(iced::widget::container(tab).style(move |_t| container_style(is_active)));
        }
        bar = bar.push(
            button("+ New")
                .on_press(Message::NewFile)
                .padding([4.0, 8.0]),
        );
        iced::widget::container(bar)
            .width(Fill)
            .padding([4.0, 8.0])
            .into()
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let Some(doc) = self.tabs.active() else {
            return iced::widget::center(text("Open a file to get started")).into();
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

    /// The terminal panel: output (ANSI-stripped tail) + a command line.
    fn terminal_view(&self) -> Element<'_, Message> {
        // Bounded tail keeps the text layout cheap.
        let mut tail: Vec<&str> = self.term_output.lines().rev().take(150).collect();
        tail.reverse();
        let faded = Color::from([0.55, 0.55, 0.6]);
        let output = scrollable(
            text(tail.join("\n"))
                .font(Font::with_name("Menlo"))
                .size(13),
        )
        .id(TERM_SCROLL_ID)
        .anchor_bottom()
        .width(Length::Fill)
        .height(Length::Fixed(190.0));
        let input = text_input("Type a command…", &self.term_input)
            .font(Font::with_name("Menlo"))
            .size(13)
            .on_input(Message::TermInput)
            .on_submit(Message::TermSubmit(self.term_input.clone()));
        let content = iced::widget::column![
            text("Terminal — line mode; full-screen apps (vim, less) will garble")
                .size(11)
                .color(faded),
            output,
            iced::widget::row![text("❯").size(13).color(faded), input].spacing(6),
        ]
        .spacing(6)
        .padding(10)
        .align_x(iced::Alignment::Start);
        iced::widget::container(content)
            .width(Length::Fill)
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(Color::from([0.09, 0.1, 0.11]))),
                ..Default::default()
            })
            .into()
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
                text(left).size(11).color(faded),
                iced::widget::space::horizontal(),
                text(status).size(11).color(Color::from([1.0, 0.45, 0.45])),
            ]
            .padding([4, 10])
        } else {
            iced::widget::row![
                text(left).size(11).color(faded),
                iced::widget::space::horizontal(),
            ]
            .padding([4, 10])
        };
        iced::widget::container(content).width(Fill).into()
    }
}

/// Tab-style for a tab: highlighted when active.
fn container_style(active: bool) -> iced::widget::container::Style {
    let bg = if active {
        [0.22, 0.24, 0.28]
    } else {
        [0.14, 0.15, 0.17]
    };
    iced::widget::container::Style {
        background: Some(iced::Background::Color(Color::from(bg))),
        ..Default::default()
    }
}

/// Two *identical* highlight format functions. Passing a different fn
/// pointer makes the widget think the highlighter changed and re-highlight
/// every line — needed after undo/redo/tab switch replaces the buffer.
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

/// $HOME (or /), for the terminal's starting directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Diff a text input's whole new value against the old one:
/// insertions are the suffix, deletions become DEL bytes, anything else
/// (e.g. a pasted selection) is sent whole.
fn term_input_diff(old: &str, new: &str) -> Vec<u8> {
    if old == new {
        return Vec::new();
    }
    if let Some(suffix) = new.strip_prefix(old) {
        return suffix.as_bytes().to_vec();
    }
    if let Some(removed) = old.strip_prefix(new) {
        return vec![0x7f; removed.chars().count()];
    }
    new.as_bytes().to_vec()
}

/// Native yes/no; returns `true` to proceed (i.e. the user chose Yes).
fn confirm_discard() -> bool {
    let answer = rfd::MessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("This file has unsaved changes.\nDiscard them and continue?")
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    answer == rfd::MessageDialogResult::Yes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_diff_insertion() {
        assert_eq!(term_input_diff("", "ls"), b"ls".to_vec());
        assert_eq!(term_input_diff("ls", "ls -l"), b" -l".to_vec());
        assert!(term_input_diff("ls", "ls").is_empty());
    }

    #[test]
    fn input_diff_backspace_counts_chars() {
        assert_eq!(term_input_diff("abc", "ab"), vec![0x7f]);
        assert_eq!(term_input_diff("aé", "a"), vec![0x7f]); // 2-byte char, one DEL
    }

    #[test]
    fn input_diff_replacement_sends_whole() {
        assert_eq!(term_input_diff("abc", "xyz"), b"xyz".to_vec());
    }
}
