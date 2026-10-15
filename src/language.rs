//! Language detection and the per-language highlighting settings.

/// The language a document is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Plain,
    Python,
    C,
    Java,
}

impl Language {
    /// Detects the language from a file extension.
    ///
    /// Python: `.py`, `.pyw`; C: `.c`, `.h`; Java: `.java`.
    /// Anything else is plain text.
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("py" | "pyw") => Self::Python,
            Some("c" | "h") => Self::C,
            Some("java") => Self::Java,
            _ => Self::Plain,
        }
    }

    /// The token that selects this language's grammar in the highlighter.
    ///
    /// No leading dot: syntect matches tokens against extensions like `"py"`,
    /// so `".py"` would silently fall back to plain text (no colors).
    pub fn token(self) -> &'static str {
        match self {
            Self::Plain => "text/plain",
            Self::Python => "py",
            Self::C => "c",
            Self::Java => "java",
        }
    }

    /// The human-readable name, shown in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain Text",
            Self::Python => "Python",
            Self::C => "C",
            Self::Java => "Java",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_known_extensions() {
        assert_eq!(
            Language::from_path(Path::new("a/b/script.py")),
            Language::Python
        );
        assert_eq!(Language::from_path(Path::new("gui.pyw")), Language::Python);
        assert_eq!(Language::from_path(Path::new("main.c")), Language::C);
        assert_eq!(Language::from_path(Path::new("lib.h")), Language::C);
        assert_eq!(Language::from_path(Path::new("App.java")), Language::Java);
    }

    #[test]
    fn falls_back_to_plain() {
        assert_eq!(Language::from_path(Path::new("README.md")), Language::Plain);
        assert_eq!(Language::from_path(Path::new("Makefile")), Language::Plain);
        assert_eq!(
            Language::from_path(Path::new("archive.tar")),
            Language::Plain
        );
    }

    #[test]
    fn tokens_select_grammars() {
        for lang in [
            Language::Plain,
            Language::Python,
            Language::C,
            Language::Java,
        ] {
            assert!(!lang.token().is_empty());
            assert!(!lang.label().is_empty());
        }
    }
}
