//! Tabs for opened documents: the "opened editors" section.
//!
//! Opening a file never replaces the current buffer: it activates the tab if
//! the file is already open, otherwise it adds a new tab. This is what makes
//! "open a file, then a folder" feel sane.

use crate::document::Document;
use std::path::Path;

#[derive(Debug)]
pub struct Tabs {
    pub docs: Vec<Document>,
    pub active: Option<usize>,
}

impl Tabs {
    pub fn with_untitled() -> Self {
        Self {
            docs: vec![Document::untitled()],
            active: Some(0),
        }
    }

    pub fn active(&self) -> Option<&Document> {
        self.active.and_then(|i| self.docs.get(i))
    }

    pub fn active_mut(&mut self) -> Option<&mut Document> {
        let i = self.active?;
        self.docs.get_mut(i)
    }

    /// Returns the file name (or "untitled") for tab labels.
    pub fn name(&self, i: usize) -> String {
        self.docs
            .get(i)
            .and_then(|d| d.path.as_ref().and_then(|p| p.file_name()))
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string())
    }

    /// Opens a file as a tab. Already open → activate it (in-memory edits
    /// win over disk). Returns `true` if a new tab was added.
    pub fn open_file(&mut self, path: &Path) -> std::io::Result<bool> {
        if let Some(i) = self
            .docs
            .iter()
            .position(|d| d.path.as_deref() == Some(path))
        {
            self.active = Some(i);
            Ok(false)
        } else {
            let doc = Document::open(path).map_err(|e| std::io::Error::other(e.to_string()))?;
            self.docs.push(doc);
            self.active = Some(self.docs.len() - 1);
            Ok(true)
        }
    }

    /// Adds a new untitled tab and makes it active.
    pub fn new_untitled(&mut self) {
        self.docs.push(Document::untitled());
        self.active = Some(self.docs.len() - 1);
    }

    /// Closes tab `i`. The editor never becomes empty: closing the last tab
    /// leaves a fresh untitled one.
    pub fn close(&mut self, i: usize) {
        if self.docs.get(i).is_none() {
            return;
        }
        self.docs.remove(i);
        if self.docs.is_empty() {
            self.docs.push(Document::untitled());
            self.active = Some(0);
        } else {
            let len = self.docs.len();
            self.active = Some(self.active.map_or(len - 1, |a| {
                if a == i {
                    len - 1
                } else if a > i {
                    a - 1
                } else {
                    a
                }
            }));
        }
    }

    /// True if any tab would lose data if force-closed.
    pub fn any_dirty(&self) -> bool {
        self.docs.iter().any(|d| d.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"hello").unwrap();
        p
    }

    #[test]
    fn open_same_file_twice_yields_one_tab() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(dir.path(), "a.txt");
        let mut tabs = Tabs::with_untitled();
        assert!(tabs.open_file(&p).unwrap());
        assert!(!tabs.open_file(&p).unwrap()); // activate, not duplicate
        assert_eq!(tabs.docs.len(), 2);
        assert_eq!(tabs.active, Some(1));
    }

    #[test]
    fn close_active_moves_selection() {
        let dir = tempfile::tempdir().unwrap();
        let pa = file(dir.path(), "a.txt");
        let pb = file(dir.path(), "b.txt");
        let mut tabs = Tabs::with_untitled();
        tabs.open_file(&pa).unwrap();
        tabs.open_file(&pb).unwrap();
        assert_eq!(tabs.active, Some(2));
        tabs.close(2);
        assert_eq!(tabs.active, Some(1)); // falls back to last tab
        tabs.close(1);
        assert_eq!(tabs.active, Some(0)); // index shifted
        assert_eq!(tabs.docs.len(), 1);
    }

    #[test]
    fn closing_last_tab_leaves_untitled() {
        let mut tabs = Tabs::with_untitled();
        tabs.close(0);
        assert_eq!(tabs.docs.len(), 1);
        assert!(tabs.docs[0].path.is_none());
        assert_eq!(tabs.active, Some(0));
    }
}
