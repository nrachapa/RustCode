# RustCode

A lightweight native code editor for macOS (Apple Silicon), written in Rust on
[iced](https://github.com/iced-rs/iced) 0.14 (wgpu renderer). Edit Python, C,
and Java files with syntax highlighting, fast typing, and reliable file saving.

Not an IDE: no terminal, debugger, LSP, or extension system (by design, for now).

## Build and run

Prerequisite: a Rust toolchain (`rustup` installs one). No other setup needed —
all dependencies are crates.

```sh
cargo run            # debug build (fast to compile, slower app)
cargo run --release  # release build (recommended for use)
```

The binary is a native arm64 Mach-O executable (no Rosetta):

```sh
file target/release/rustcode   # → Mach-O 64-bit executable arm64
```

## Tests

```sh
cargo test           # 19 unit tests: language detection, atomic save,
                     # binary/UTF-8 rejection, undo/redo, workspace tree
                     # (sort, lazy expand, create/rename/delete, refresh)
cargo clippy --all-targets
```

Tests cover the core (document, save, language) via the library crate.
**GUI behavior is not automated** — iced has no practical UI-test harness.
Use the manual checklist below for every meaningful change.

## Manual GUI checklist

1. Launch → dark window, empty editable area, toolbar (New / Open… / Save / Save As…).
2. Type text: cursor blinks, selection works, Cmd/Ctrl drag selects.
3. Open a `.py`, a `.c`, and a `.java` file (toolbar **Open…**): each gets distinct syntax coloring; toolbar label shows filename + language.
4. Edit a line → `•` appears next to the filename. Save (button or **⌘S**) → dot clears.
5. Edit, then press **New** or **Open…**: "Discard unsaved changes?" dialog appears; *No* keeps your edits.
6. **Save As…** on an untitled buffer → native save panel; the file is created with the right extension and language.
7. Open a binary file (e.g. a `.png`): an error appears at the bottom; the editor is not corrupted.
8. Save a file to a read-only location → a "Save failed: …" message appears; the original file is untouched (saves are atomic: temp file + rename).
9. Resize the window; long unindented lines scroll horizontally without wrapping.
10. **⌘O** opens the file picker from the keyboard.
11. **Tab** inserts four spaces (not a raw tab character); **⌘/Ctrl+Z** undoes, **⇧⌘Z** / **⌘Y** redoes — syntax colors stay correct after each.
12. **Open Folder…** (toolbar) → the sidebar shows the folder's tree. Click a folder to expand/collapse; click a file to open it. The New File / New Folder / Rename / Delete / Refresh buttons act on the selected entry (Delete confirms; directories are never deleted).

## Packaging

```sh
# ad-hoc .app bundle (unsigned)
./bundle.sh
open RustCode.app
```

Unsigned builds trigger Gatekeeper on first open: **right-click → Open → Open**.
Distribution to other machines needs a Developer ID + notarization (not done in v1).
The app is not sandboxed, so it reads/writes folders you pick in the dialogs.

## Architecture

See `docs/adr/` for the recorded decisions (framework, editing stack,
highlighting, file safety). Module layout:

```
src/
├── main.rs        # entry point
├── app.rs         # per-window state + message handling
├── toolbar.rs     # mouse-first file controls
├── explorer.rs    # file-explorer sidebar (tree + actions)
└── (lib.rs)       # testable core: document/, workspace/, language.rs
```

Planned (not yet built): multiple tabs, multiple windows, find,
auto-indent, bracket matching, settings persistence, worker pool
for off-thread file I/O.

## Current limitations

- Single window, single document (tabs and multi-window are Stages 2–3).
- The explorer lists one folder; there is no project tree across folders yet.
- File dialogs and folder I/O block the UI thread while open (worker pool is Stage 2).
- Idle RSS ≈ 95 MB (wgpu + text stack); under the 100 MB target, barely.
- Highlighting is regex-grade (syntect via iced's built-in highlighter), not a full grammar.
