//! Document state: the in-memory buffer for one open file.

pub mod save;

use crate::language::Language;
use iced::widget::text_editor::{Content, Cursor};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A file's contents held in memory: text, path, language, and dirty state.
pub struct Document {
    /// Where this document lives on disk. `None` for an untitled buffer.
    pub path: Option<PathBuf>,
    /// The editable text. Shared with the editor widget via `RefCell`.
    pub content: Content,
    pub language: Language,
    /// True after an edit that has not yet been saved.
    pub dirty: bool,
    /// Undo history: pre-edit snapshots (text + cursor), oldest first.
    undo: Vec<Snapshot>,
    /// Redo history, oldest first.
    redo: Vec<Snapshot>,
    /// When the last edit was made, for coalescing runs of typing.
    last_edit: Option<(Instant, EditKind)>,
}

/// One pre-edit snapshot. Undo replaces the buffer with it.
#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: Cursor,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("path", &self.path)
            .field("dirty", &self.dirty)
            .finish()
    }
}

/// Which kind of edit was last performed; consecutive keystrokes
/// of the same kind coalesce into one undo step.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

/// Snapshot cap — 100 full-buffer snapshots.
const UNDO_CAP: usize = 100;
/// Edits closer together than this (and of the same kind) are one step.
const COALESCE_WINDOW: Duration = Duration::from_millis(1200);

/// Why a file could not be opened as a text document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// The file could not be read (missing, permission, ...).
    Io(std::io::ErrorKind),
    /// The file contains NUL bytes; it is binary, not source code.
    Binary,
    /// The file is not valid UTF-8.
    NotUtf8,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(kind) => write!(f, "cannot read file ({kind:?})"),
            Self::Binary => f.write_str(
                "file looks like binary data, not text — RustCode cannot edit it as source code",
            ),
            Self::NotUtf8 => f.write_str("file is not valid UTF-8"),
        }
    }
}

impl Document {
    /// An empty, unsaved buffer.
    pub fn untitled() -> Self {
        Self {
            path: None,
            content: Content::default(),
            language: Language::Plain,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        }
    }

    /// Reads a file into a new document.
    ///
    /// Rejects binary files (NUL in the first 8 KiB) and invalid UTF-8
    /// rather than corrupting them.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let data = std::fs::read(path).map_err(|e| OpenError::Io(e.kind()))?;
        if data
            .get(..data.len().min(8192))
            .is_some_and(|head| head.contains(&0))
        {
            return Err(OpenError::Binary);
        }
        let text = String::from_utf8(data).map_err(|_| OpenError::NotUtf8)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            content: Content::with_text(&text),
            language: Language::from_path(path),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        })
    }

    /// Applies an editor action. The iced widget only *publishes* actions;
    /// the app performs them on the shared buffer. Editing actions also
    /// record an undo point (coalescing fast runs of the same kind) and
    /// mark the document dirty; motion/click/scroll do not.
    pub fn edit(&mut self, action: iced::widget::text_editor::Action) {
        use iced::widget::text_editor::{Action, Edit};

        let kind = match &action {
            Action::Edit(Edit::Backspace | Edit::Delete) => Some(EditKind::Deleting),
            Action::Edit(Edit::Insert(_) | Edit::Paste(_) | Edit::Enter) => Some(EditKind::Typing),
            Action::Edit(_) => Some(EditKind::Other),
            // Motion / click / drag / scroll: performed, but never an undo point.
            _ => None,
        };

        if let Some(kind) = kind {
            let now = Instant::now();
            let coalesce = matches!(
                &self.last_edit,
                Some((t, k)) if k == &kind && now.duration_since(*t) < COALESCE_WINDOW
            );
            if !coalesce {
                self.undo.push(Snapshot {
                    text: self.content.text(),
                    cursor: self.content.cursor(),
                });
                if self.undo.len() > UNDO_CAP {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            self.last_edit = Some((now, kind));
            self.dirty = true;
        }

        self.content.perform(action);
    }

    /// Undo the last edit step. Returns `true` if something was undone.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            text: self.content.text(),
            cursor: self.content.cursor(),
        });
        self.restore(snap);
        true
    }

    /// Redo the last undone step. Returns `true` if something was redone.
    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            text: self.content.text(),
            cursor: self.content.cursor(),
        });
        self.restore(snap);
        true
    }

    fn restore(&mut self, snap: Snapshot) {
        self.content = Content::with_text(&snap.text);
        self.content.move_to(snap.cursor);
        self.dirty = true;
        self.last_edit = None;
    }

    /// The display name: the file name, or "untitled".
    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    /// A full copy of the text (used for saving and searching).
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// When the last edit happened — for autosave debounce.
    pub fn last_edit_at(&self) -> Option<Instant> {
        self.last_edit.map(|(t, _)| t)
    }

    /// The text as it should hit disk: ensures the final line ending.
    pub fn text_for_saving(&self) -> String {
        let mut text = self.content.text();
        if let Some(ending) = self.content.line_ending()
            && !text.is_empty()
            && !text.ends_with(ending.as_str())
        {
            text.push_str(ending.as_str());
        }
        text
    }

    /// Atomically saves to the current path.
    pub fn save(&mut self) -> Result<(), std::io::Error> {
        let path = self.path.clone().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no file path — use Save As")
        })?;
        save::atomic(&path, &self.text())?;
        self.dirty = false;
        Ok(())
    }

    /// Points the document at a new path (after Save As), re-detecting the language.
    pub fn set_path(&mut self, path: PathBuf) {
        self.language = Language::from_path(&path);
        self.path = Some(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        // Keep the tempdir alive for the process; tests only read.
        std::mem::forget(dir);
        path
    }

    #[test]
    fn opens_text_file_and_detects_language() {
        let path = tmp_file("script.py", b"print('hi')\n");
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.language, Language::Python);
        assert!(!doc.dirty);
        assert_eq!(doc.title(), "script.py");
        assert_eq!(doc.text(), "print('hi')\n");
        assert_eq!(doc.content.line_count(), 2);
    }

    #[test]
    fn rejects_binary_files() {
        let path = tmp_file("blob.bin", b"\x00\x01\x02ELF");
        assert!(matches!(Document::open(&path), Err(OpenError::Binary)));
    }

    #[test]
    fn rejects_invalid_utf8() {
        let path = tmp_file("bad.txt", &[0xff, 0xfe, 0x01, 0x02]);
        assert!(matches!(Document::open(&path), Err(OpenError::NotUtf8)));
    }

    #[test]
    fn missing_file_is_io_error() {
        let path = std::path::Path::new("/nonexistent/rustcode/nope.txt");
        assert!(matches!(
            Document::open(path),
            Err(OpenError::Io(std::io::ErrorKind::NotFound))
        ));
    }

    #[test]
    fn untitled_document_has_no_path() {
        let doc = Document::untitled();
        assert!(doc.path.is_none());
        assert!(doc.text().is_empty());
        assert_eq!(doc.title(), "untitled");
    }

    #[test]
    fn undo_redo_round_trip() {
        use iced::widget::text_editor::{Action, Edit};
        let mut doc = Document::untitled();
        doc.edit(Action::Edit(Edit::Insert('a')));
        assert_eq!(doc.text(), "a");
        doc.edit(Action::Edit(Edit::Backspace));
        assert_eq!(doc.text(), "");
        doc.edit(Action::Edit(Edit::Insert('b')));
        assert_eq!(doc.text(), "b");

        assert!(doc.undo());
        assert_eq!(doc.text(), "");
        assert!(doc.undo());
        assert_eq!(doc.text(), "a");
        assert!(doc.redo());
        assert_eq!(doc.text(), "");
        assert!(doc.redo());
        assert_eq!(doc.text(), "b");
    }

    #[test]
    fn rapid_typing_coalesces_into_one_step() {
        use iced::widget::text_editor::{Action, Edit};
        let mut doc = Document::untitled();
        for c in ['a', 'b', 'c'] {
            doc.edit(Action::Edit(Edit::Insert(c)));
        }
        assert_eq!(doc.text(), "abc");
        // Three fast keystrokes = one undo step back to the start.
        assert!(doc.undo());
        assert_eq!(doc.text(), "");
        assert!(!doc.undo());
    }

    #[test]
    fn motion_is_not_an_undo_point() {
        use iced::widget::text_editor::{Action, Edit, Motion};
        let mut doc = Document::untitled();
        doc.edit(Action::Edit(Edit::Insert('x')));
        let pre = doc.content.cursor().position.column;
        doc.edit(Action::Move(Motion::Left));
        // Motion is performed (cursor moves)…
        assert_eq!(doc.content.cursor().position.column, pre - 1);
        assert!(doc.undo());
        assert_eq!(doc.text(), "");
        assert!(!doc.undo()); // …but records no undo point
    }
}
