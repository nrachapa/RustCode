//! RustCode: a lightweight native code editor for macOS.
//!
//! The framework-agnostic core (documents, saving, language detection)
//! lives here so it can be unit-tested without a GUI.

pub mod document;
pub mod language;
pub mod tabs;
pub mod terminal;
pub mod workspace;
