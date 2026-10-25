# RustCode

A lightweight native code editor for macOS (Apple Silicon), written in Rust on
[iced](https://github.com/iced-rs/iced) 0.14 (wgpu renderer). Edit Python, C,
and Java files with syntax highlighting, multiple open tabs, autosave, a
file-explorer sidebar, and a terminal panel.

Not a full IDE: no debugger, LSP, or extension system. The terminal is a
real xterm/VT100 terminal (vim and less work); it has no tabs yet.

- **Tabs**: opening a file adds a tab (or focuses the existing one); tabs are
  never silently discarded — closing a dirty tab asks first.
- **Autosave**: two seconds after you stop typing, every open file is saved
  silently (untitled buffers still need Save As…).
- **Terminal**: a real `$SHELL` on a pty, running in the opened folder
  (or `$HOME`). Type commands in the bottom panel.

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
cargo test           # unit tests: language detection, atomic save,
                     # binary/UTF-8 rejection, undo/redo, workspace tree,
                     # tabs, VT escape-sequence handling (C0/CSI/SGR), key mapping
make check           # fmt + clippy (-D warnings) + test, in one go
cargo clippy --all-targets
```

Tests cover the core (document, save, language) via the library crate.
**GUI behavior is not automated** — iced has no practical UI-test harness.
Use the manual checklist below for every meaningful change.

## Manual GUI checklist

1. Launch → dark window, tab bar with one untitled tab, toolbar, empty editable area.
2. Type text: cursor blinks, selection works, Cmd/Ctrl drag selects.
3. Open a `.py`, a `.c`, and a `.java` file (toolbar **Open…**): each gets its own tab with distinct syntax coloring; click tabs to switch between them; the `×` closes one (dirty tabs ask first).
4. Edit a line → `●` appears on the tab. Wait ~2 s after your last keystroke: the dot clears on its own (autosave); check the file on disk. **⌘S** still saves immediately.
5. **Save As…** on an untitled buffer → native save panel; the file is created with the right extension and language.
6. Open a binary file (e.g. a `.png`): an error appears at the bottom; no tab is added; existing tabs are untouched.
7. Save a file to a read-only location → a "Save failed: …" message appears; the original file is untouched (saves are atomic: temp file + rename).
8. Resize the window; long unindented lines scroll horizontally without wrapping.
9. **⌘O** opens the file picker from the keyboard; **Tab** inserts four spaces; **⌘/Ctrl+Z** undoes, **⇧⌘Z** / **⌘Y** redoes — colors stay correct after each and after switching tabs.
10. **Open Folder…** (toolbar) → the sidebar shows the folder's tree. Click a folder to expand/collapse; click a file to open it in a tab (opening a file, then a folder, loses nothing — the file keeps its tab). The New File / New Folder / Rename / Delete / Refresh buttons act on the selected entry (Delete confirms; directories are never deleted).
11. **Terminal** (toolbar) → a bottom panel with a live shell in the opened folder. **Click it** to type — keys go to the shell while it has focus, to the editor otherwise (the block cursor shows which has it). Full-screen apps (vim, less) now work; ⌘V pastes (bracketed), the wheel scrolls the 10,000-line history, `exit` restarts the shell.

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
├── app.rs         # per-window state: tabs, terminal, autosave, layout
├── toolbar.rs     # mouse-first file controls
├── explorer.rs    # file-explorer sidebar (tree + actions)
├── terminal_view.rs # terminal canvas: renders the alacritty grid, keys, scroll
└── (lib.rs)       # testable core: document/, workspace/, tabs.rs,
                   # terminal.rs (pty + alacritty VT grid), language.rs
docs/iced-0.14-api-notes.md   # verified iced 0.14 API (read before widget work)
```

Planned (not yet built): multiple windows, find, auto-indent, bracket
matching, settings persistence, worker pool for off-thread file I/O.

## Current limitations

- Single window (multiple windows is Stage 3).
- The explorer lists one folder; there is no project tree across folders yet.
- Terminal: 80×30 alacritty grid (~10,000 lines of scroll history, not
  all visible in the 80×30 viewport); the shell restarts on `exit` and
  when a new folder is opened; no terminal tabs yet.
- Autosave runs on a 2 s debounce after typing; it saves only files that
  already have a path.
- File dialogs and folder I/O block the UI thread while open (worker pool is Stage 2).
- Idle RSS ≈ 97 MB (wgpu + text stack + shell); under the 100 MB target, barely.
- Highlighting is regex-grade (syntect via iced's built-in highlighter), not a full grammar.
