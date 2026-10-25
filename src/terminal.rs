//! Terminal: a real VT terminal.
//!
//! Emulation is alacritty's `Term` (vte state machine + cell grid, exactly
//! what lapce embeds); the shell runs on a pty (`portable-pty`). The UI
//! renders the cell grid (see `terminal_view`), and keys are mapped to byte
//! sequences the way alacritty's front end does.

use alacritty_terminal::{
    Term,
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::test::TermSize,
    vte::ansi::{self, Color, Rgb},
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::path::Path;

/// Fixed terminal size; a resizable pane is a follow-up.
pub const TERM_COLS: usize = 80;
pub const TERM_ROWS: usize = 30;

/// Events the terminal exchanges with the UI.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Raw output bytes from the shell.
    Output(Vec<u8>),
    /// Bytes the emulator wants written to the shell (e.g. mouse reports).
    PtyWrite(Vec<u8>),
    /// The shell process exited.
    Exited,
}

/// Forwards the emulator's `PtyWrite` requests to the UI over a channel.
#[derive(Clone)]
pub struct PtyProxy {
    tx: futures::channel::mpsc::UnboundedSender<TermEvent>,
}

impl EventListener for PtyProxy {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(s) = event {
            let _ = self.tx.unbounded_send(TermEvent::PtyWrite(s.into_bytes()));
        }
    }
}

/// A shell running on a pty.
pub struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Session {
    /// Spawns `$SHELL` (or /bin/zsh) with the given working directory.
    pub fn spawn(cwd: &Path) -> std::io::Result<Self> {
        let pts = native_pty_system();
        let pair = pts
            .openpty(PtySize {
                rows: TERM_ROWS as u16,
                cols: TERM_COLS as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(cwd);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            master: pair.master,
            writer,
            child,
        })
    }

    /// A readable handle for the reader thread.
    pub fn reader(&self) -> std::io::Result<Box<dyn std::io::Read + Send>> {
        self.master
            .try_clone_reader()
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    pub fn write(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(input)?;
        self.writer.flush()
    }

    pub fn kill(&mut self) {
        self.child.kill().ok();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The terminal: emulator + shell + output plumbing.
pub struct Terminal {
    pub term: Term<PtyProxy>,
    parser: ansi::Processor,
    pub session: Option<Session>,
    /// Cloned into the proxy and the reader thread.
    pub tx: futures::channel::mpsc::UnboundedSender<TermEvent>,
    /// The reader thread stays alive while the shell runs.
    _reader: Option<std::thread::JoinHandle<()>>,
}

impl Terminal {
    /// Spawns the shell in `cwd` and a matching emulator grid.
    pub fn spawn(
        cwd: &Path,
        tx: futures::channel::mpsc::UnboundedSender<TermEvent>,
    ) -> std::io::Result<Self> {
        let session = Session::spawn(cwd)?;
        let config = alacritty_terminal::term::Config {
            scrolling_history: 1000,
            ..Default::default()
        };
        let term = Term::new(
            config,
            &TermSize::new(TERM_COLS, TERM_ROWS),
            PtyProxy { tx: tx.clone() },
        );
        let mut terminal = Self {
            term,
            parser: ansi::Processor::new(),
            session: Some(session),
            tx,
            _reader: None,
        };
        terminal.spawn_reader();
        Ok(terminal)
    }

    fn spawn_reader(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Ok(mut reader) = session.reader() else {
            return;
        };
        let tx = self.tx.clone();
        self._reader = Some(std::thread::spawn(move || {
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
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        // Unix ptys report EOF this way.
                        let _ = tx.unbounded_send(TermEvent::Exited);
                        break;
                    }
                    Err(_) => break,
                }
            }
        }));
    }

    /// Feeds shell output through the VT state machine.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Sends input bytes to the shell.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if let Some(session) = &mut self.session {
            session.write(bytes).ok();
        }
    }

    /// Whether the shell has bracketed paste mode on (vim et al.).
    pub fn bracketed_paste(&self) -> bool {
        self.term
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// Scrolls the display by whole lines.
    pub fn scroll(&mut self, delta_lines: i32) {
        if delta_lines != 0 {
            self.term.scroll_display(Scroll::Delta(delta_lines));
        }
    }

    /// Restarts the shell (new cwd); the emulator history is kept.
    pub fn restart_shell(&mut self, cwd: &Path) {
        match Session::spawn(cwd) {
            Ok(session) => {
                self.session = Some(session);
                self.spawn_reader();
            }
            Err(e) => eprintln!("terminal: {e}"),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.kill()
        }
    }
}

/// Maps a key event to the byte sequence a terminal expects, ported from
/// alacritty/lapce's keyboard handling. `key` should be the *modified* key
/// (shift applied), `modifiers` the full modifier state.
pub fn key_bytes(
    key: &iced_core::keyboard::Key,
    modifiers: &iced_core::keyboard::Modifiers,
) -> Option<Vec<u8>> {
    use iced_core::keyboard::Key;

    match key.as_ref() {
        Key::Character(c) => {
            if modifiers.control() && !modifiers.alt() {
                // Ctrl+letter → C0 control character.
                let c = c.chars().next()?;
                let s = match c.to_ascii_lowercase() {
                    'a' => "\x01",
                    'b' => "\x02",
                    'c' => "\x03",
                    'd' => "\x04",
                    'e' => "\x05",
                    'f' => "\x06",
                    'g' => "\x07",
                    'h' => "\x08",
                    'i' => "\x09",
                    'j' => "\x0a",
                    'k' => "\x0b",
                    'l' => "\x0c",
                    'm' => "\x0d",
                    'n' => "\x0e",
                    'o' => "\x0f",
                    'p' => "\x10",
                    'q' => "\x11",
                    'r' => "\x12",
                    's' => "\x13",
                    't' => "\x14",
                    'u' => "\x15",
                    'v' => "\x16",
                    'w' => "\x17",
                    'x' => "\x18",
                    'y' => "\x19",
                    'z' => "\x1a",
                    ' ' => "\x00",
                    '[' => "\x1b",
                    '\\' => "\x1c",
                    ']' => "\x1d",
                    '^' => "\x1e",
                    '_' => "\x1f",
                    _ => return None,
                };
                Some(s.as_bytes().to_vec())
            } else if modifiers.alt() {
                // Alt+letter → ESC prefix.
                let mut v = vec![0x1b];
                v.extend_from_slice(c.as_bytes());
                Some(v)
            } else {
                let v = c.as_bytes().to_vec();
                (!v.is_empty()).then_some(v)
            }
        }
        Key::Named(named) => named_bytes(named, modifiers),
        _ => None,
    }
}

/// CSI modifier value: 1 + shift + 2·alt + 4·ctrl (xterm convention).
fn modval(modifiers: &iced_core::keyboard::Modifiers) -> u8 {
    1 + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control())
}

/// Named (non-character) keys → escape sequences, with xterm modifier bits.
fn named_bytes(
    named: iced_core::keyboard::key::Named,
    modifiers: &iced_core::keyboard::Modifiers,
) -> Option<Vec<u8>> {
    use iced_core::keyboard::key::Named as N;
    let m = modval(modifiers);

    let (base, fin): (&str, &str) = match named {
        N::Backspace => ("\x7f", ""),
        N::Enter => ("\r", ""),
        N::Escape => ("\x1b", ""),
        N::Tab => ("\x09", ""),
        N::ArrowUp => ("\x1b[A", "A"),
        N::ArrowDown => ("\x1b[B", "B"),
        N::ArrowRight => ("\x1b[C", "C"),
        N::ArrowLeft => ("\x1b[D", "D"),
        N::Home => ("\x1b[H", "H"),
        N::End => ("\x1b[F", "F"),
        N::Delete => ("\x1b[3~", "~"),
        N::Insert => ("\x1b[2~", "~"),
        N::PageUp => ("\x1b[5~", "~"),
        N::PageDown => ("\x1b[6~", "~"),
        _ => return None,
    };

    if m == 1 {
        Some(base.as_bytes().to_vec())
    } else if base.len() == 3 && !fin.is_empty() {
        // Arrows and Home/End: the 1;N form.
        Some(format!("\x1b[1;{m}{fin}").into_bytes())
    } else if fin == "~" {
        // Tilde sequences: N~.
        Some(format!("\x1b[{m}~").into_bytes())
    } else if modifiers.alt() {
        // Backspace/Enter/Esc/Tab: alt prefixes ESC.
        let mut v = vec![0x1b];
        v.extend_from_slice(base.as_bytes());
        Some(v)
    } else {
        Some(base.as_bytes().to_vec())
    }
}

/// Paste text: wrapped in 200~/201~ markers when the shell has bracketed
/// paste mode on (xterm mode 2004, set by vim et al.).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend_from_slice(text.as_bytes());
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

/// One contiguous stretch of a line sharing one foreground color.
#[derive(Debug, Clone)]
pub struct Run {
    pub col: usize,
    pub text: String,
    pub fg: Rgb,
    pub bg: Rgb,
}

/// One visible row of the terminal grid.
#[derive(Debug, Clone)]
pub struct TermLine {
    pub row: usize,
    pub runs: Vec<Run>,
}

/// Fallback for the 16 named ANSI colors until the shell sets them (OSC 4).
const ANSI_FALLBACK: [Rgb; 16] = [
    Rgb {
        r: 0x36,
        g: 0x3a,
        b: 0x41,
    }, // black
    Rgb {
        r: 0xf2,
        g: 0x6d,
        b: 0x50,
    }, // red
    Rgb {
        r: 0xa1,
        g: 0xbd,
        b: 0x59,
    }, // green
    Rgb {
        r: 0xfd,
        g: 0xd0,
        b: 0x5b,
    }, // yellow
    Rgb {
        r: 0x3f,
        g: 0x78,
        b: 0xb1,
    }, // blue
    Rgb {
        r: 0xd3,
        g: 0x86,
        b: 0x9b,
    }, // magenta
    Rgb {
        r: 0x81,
        g: 0xa2,
        b: 0xbe,
    }, // cyan
    Rgb {
        r: 0x5c,
        g: 0x67,
        b: 0x73,
    }, // white
    Rgb {
        r: 0x95,
        g: 0xa5,
        b: 0xa6,
    }, // bright black
    Rgb {
        r: 0xf2,
        g: 0x6d,
        b: 0x50,
    },
    Rgb {
        r: 0xa1,
        g: 0xbd,
        b: 0x59,
    },
    Rgb {
        r: 0xfd,
        g: 0xd0,
        b: 0x5b,
    },
    Rgb {
        r: 0xb2,
        g: 0xc7,
        b: 0xe6,
    },
    Rgb {
        r: 0xd3,
        g: 0x86,
        b: 0x9b,
    },
    Rgb {
        r: 0x81,
        g: 0xa2,
        b: 0xbe,
    },
    Rgb {
        r: 0x95,
        g: 0xa5,
        b: 0xa6,
    },
];

pub const DEFAULT_FG: Rgb = Rgb {
    r: 0xc7,
    g: 0xcc,
    b: 0xd1,
};
pub const DEFAULT_BG: Rgb = Rgb {
    r: 0x1e,
    g: 0x21,
    b: 0x26,
};

/// Resolves an SGR color against the terminal palette, with fallbacks.
fn resolve(color: &Color) -> Rgb {
    match color {
        Color::Spec(rgb) => *rgb,
        Color::Indexed(i) => {
            // 0..16 named, 16..232 cube, 232..256 ramp.
            if (*i as usize) < 16 {
                ANSI_FALLBACK[*i as usize]
            } else if (16..232).contains(&(*i as usize)) {
                let i = *i as usize - 16;
                let r5 = (i >> 5) & 7;
                let g5 = (i >> 2) & 7;
                let b5 = i & 7;
                let f = |v: u32| if v == 0 { 0 } else { 55 + 40 * v };
                Rgb {
                    r: f(r5 as u32) as u8,
                    g: f(g5 as u32) as u8,
                    b: f(b5 as u32) as u8,
                }
            } else if (232..256).contains(&(*i as usize)) {
                let v = (8 + 10 * (*i as usize - 232)) as u8;
                Rgb { r: v, g: v, b: v }
            } else {
                DEFAULT_FG
            }
        }
        Color::Named(n) => {
            let i = *n as usize;
            if i < 16 { ANSI_FALLBACK[i] } else { DEFAULT_FG }
        }
    }
}
impl Terminal {
    /// The visible viewport as colored runs, ready for the canvas.
    pub fn lines(&self) -> Vec<TermLine> {
        let grid = self.term.grid();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let mut out = Vec::new();
        for (i, line_no) in (top..=bottom).enumerate() {
            let row = &grid[Line(line_no)];
            let len = row.len().min(TERM_COLS);
            let mut runs = Vec::new();
            let mut cur: Option<Run> = None;
            for col in 0..len {
                let cell = &row[Column(col)];
                let fg = resolve(&cell.fg);
                let bg = resolve(&cell.bg);
                match cur.as_mut() {
                    Some(r) if r.fg == fg && r.bg == bg => {
                        r.text.push(cell.c);
                    }
                    _ => {
                        if let Some(r) = cur.take() {
                            runs.push(r);
                        }
                        let text = cell.c.to_string();
                        let mut r = Run {
                            col,
                            fg,
                            bg,
                            text: String::new(),
                        };
                        r.text = text;
                        cur = Some(r);
                    }
                }
            }
            if let Some(r) = cur.take() {
                runs.push(r);
            }
            out.push(TermLine { row: i, runs });
        }
        out
    }

    /// The cursor position in viewport coordinates (row, col), if visible.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        let rc = self.term.renderable_content();
        let p: Point = rc.cursor.point;
        let top = self.term.grid().topmost_line().0;
        let row = p.line.0 - top;
        if (0..TERM_ROWS as i32).contains(&row) {
            Some((row as usize, p.column.0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_with(bytes: &[u8]) -> Terminal {
        let (tx, _rx) = futures::channel::mpsc::unbounded::<TermEvent>();
        let session = Session::spawn(std::path::Path::new("/")).ok();
        let config = alacritty_terminal::term::Config {
            scrolling_history: 100,
            ..Default::default()
        };
        let term = Term::new(
            config,
            &TermSize::new(TERM_COLS, TERM_ROWS),
            PtyProxy { tx },
        );
        let mut t = Terminal {
            term,
            parser: ansi::Processor::new(),
            session,
            tx: {
                let (tx, _rx) = futures::channel::mpsc::unbounded();
                tx
            },
            _reader: None,
        };
        t.feed(bytes);
        t
    }

    #[test]
    fn feed_renders_text() {
        let t = term_with(b"hello");
        let lines = t.lines();
        let first: String = lines[0].runs.iter().flat_map(|r| r.text.chars()).collect();
        assert!(first.starts_with("hello"));
    }

    #[test]
    fn feed_moves_cursor_and_overwrites() {
        // "abc", cursor left two, write " X" → "a X" (overwrite in place).
        let t = term_with(b"abc\x1b[D\x1b[D X");
        let lines = t.lines();
        let first: String = lines[0].runs.iter().flat_map(|r| r.text.chars()).collect();
        assert!(first.starts_with("a X"), "got {first:?}");
    }

    #[test]
    fn clear_screen_works() {
        // On the alt screen, CSI 2J clears in place (main screen would scroll
        // the content into scrollback, which this fixed-size test grid lacks).
        let t = term_with(b"\x1b[?1049hjunk\r\ntext\x1b[2J");
        let lines = t.lines();
        let all: String = lines
            .iter()
            .flat_map(|l| l.runs.iter().flat_map(|r| r.text.chars()))
            .collect();
        assert!(
            !all.contains("junk") && !all.contains("text"),
            "got {all:?}"
        );
    }

    #[test]
    fn colors_produce_distinct_runs() {
        let t = term_with(b"\x1b[31mred\x1b[0mplain");
        let lines = t.lines();
        assert!(lines[0].runs.len() >= 2);
    }

    #[test]
    fn key_mapping_ctrl_c() {
        use iced_core::keyboard::{Key, Modifiers, key};
        let m = Modifiers::CTRL;
        assert_eq!(key_bytes(&Key::Named(key::Named::Control), &m), None);
    }

    #[test]
    fn key_mapping_basics() {
        use iced_core::keyboard::{Key, Modifiers, key};
        let none = Modifiers::empty();
        assert_eq!(
            key_bytes(&Key::Character("l".into()), &none),
            Some(b"l".to_vec())
        );
        assert_eq!(
            key_bytes(&Key::Named(key::Named::Enter), &none),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            key_bytes(&Key::Named(key::Named::ArrowUp), &none),
            Some(b"\x1b[A".to_vec())
        );
        let ctrl = Modifiers::CTRL;
        assert_eq!(
            key_bytes(&Key::Character("c".into()), &ctrl),
            Some(vec![0x03])
        );
        let shift = Modifiers::SHIFT;
        assert_eq!(
            key_bytes(&Key::Named(key::Named::ArrowUp), &shift),
            Some(b"\x1b[1;2A".to_vec())
        );
        let alt = Modifiers::ALT;
        assert_eq!(
            key_bytes(&Key::Character("a".into()), &alt),
            Some(vec![0x1b, b'a'])
        );
    }

    fn test_key(c: char) -> iced_core::keyboard::Key {
        iced_core::keyboard::Key::Character(c.to_string().into())
    }

    #[test]
    fn paste_wraps_only_in_bracketed_mode() {
        // Spec: xterm sets mode 2004; pastes arrive wrapped in 200~/201~.
        assert_eq!(paste_bytes("hello", false), b"hello".as_slice());
        assert_eq!(
            paste_bytes("hello", true),
            b"\x1b[200~hello\x1b[201~".as_slice()
        );
    }

    #[test]
    fn key_mapping_ctrl_letters() {
        use iced_core::keyboard::Modifiers;
        let ctrl = Modifiers::CTRL;
        assert_eq!(key_bytes(&test_key('c'), &ctrl), Some(vec![0x03]));
        assert_eq!(key_bytes(&test_key('z'), &ctrl), Some(vec![0x1a]));
        assert_eq!(key_bytes(&test_key('h'), &ctrl), Some(vec![0x08]));
    }
}
