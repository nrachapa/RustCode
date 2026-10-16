//! Terminal panel: a pty-backed, *line-oriented* terminal.
//!
//! Limitation: escape sequences are stripped, not interpreted, so output is
//! readable but full-screen programs (vim, less, htop) will garble.

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::path::Path;

/// Fixed pty size; the shell wraps at this width.
pub const PTY_ROWS: u16 = 50;
pub const PTY_COLS: u16 = 200;
/// Output buffer cap; the front is trimmed as output grows.
pub const OUTPUT_CAP: usize = 256 * 1024;

/// State machine that strips ANSI/VT escape sequences from a byte stream.
/// Handles sequences split across read boundaries (state persists between feeds).
#[derive(Debug, Default)]
pub struct AnsiFilter {
    /// 0 = text, 1 = in CSI, 2 = in OSC, 3 = saw ESC inside OSC (ST pending)
    state: u8,
    last_was_nl: bool,
    /// Incomplete UTF-8: pending[0..plen]
    plen: u8,
    pending: [u8; 4],
}

impl AnsiFilter {
    pub fn feed(&mut self, chunk: &[u8], out: &mut String) {
        for &b in chunk {
            match (self.state, b) {
                (0, 0x1b) => self.state = 1,
                // `]` (0x5D) is inside the CSI final-byte range, so it must be
                // matched first.
                (1, b'[') => {}                     // stay in CSI (redundant intro)
                (1, b']') => self.state = 2,        // OSC
                (1, 0x1b) => {}                     // new escape; abandon the current one
                (1, 0x20..=0x3f) => {}              // CSI params/intermediates
                (1, 0x40..=0x7e) => self.state = 0, // CSI final byte
                (1, _) => self.state = 0,           // 2-byte escape, done
                (2, 0x07) => self.state = 0,        // OSC ended by BEL
                (2, 0x1b) => self.state = 3,        // ST starts
                (2, _) => {}                        // OSC content
                (3, b'\\') => self.state = 0,       // ST complete
                (3, _) => self.state = 0,           // malformed; treat as end
                (0, 0x0d | 0x0a) => {
                    // Map CR and LF to newline; collapse CRLF into one.
                    if !self.last_was_nl {
                        out.push('\n');
                    }
                    self.last_was_nl = true;
                }
                (0, b) => {
                    self.last_was_nl = false;
                    self.push_text(b, out);
                }
                _ => {}
            }
        }
    }

    /// Incremental UTF-8 decode (a multibyte char may be split across reads).
    fn push_text(&mut self, b: u8, out: &mut String) {
        if b < 0x80 {
            self.flush_pending(out);
            out.push(b as char);
        } else if b & 0b1100_0000 == 0b1000_0000 {
            if self.plen < 4 {
                self.pending[self.plen as usize] = b;
                self.plen += 1;
            }
            self.flush_pending(out);
        } else {
            self.plen = 1;
            self.pending[0] = b;
            self.flush_pending(out);
        }
    }

    fn flush_pending(&mut self, out: &mut String) {
        if self.plen == 0 {
            return;
        }
        let need = match self.pending[0] {
            0b1100_0000..=0b1100_0111 => 2,
            0b1110_0000..=0b1110_1111 => 3,
            0b1111_0000..=0b1111_0111 => 4,
            _ => 1,
        };
        if self.plen as usize == need {
            let c = std::str::from_utf8(&self.pending[..need])
                .ok()
                .and_then(|s| s.chars().next());
            out.push(c.unwrap_or('\u{fffd}'));
            self.plen = 0;
        }
    }

    /// Discards any sequence/char left open (call when the shell exits).
    pub fn reset(&mut self) {
        self.state = 0;
        self.plen = 0;
        self.last_was_nl = false;
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
                rows: PTY_ROWS,
                cols: PTY_COLS,
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

/// Appends a pty chunk to the output buffer: ANSI-stripped, front-capped.
pub fn append_output(buf: &mut String, chunk: &[u8], filter: &mut AnsiFilter) {
    let mut clean = String::new();
    filter.feed(chunk, &mut clean);
    buf.push_str(&clean);
    if buf.len() > OUTPUT_CAP {
        let mut cut = buf.len() - OUTPUT_CAP / 2;
        while cut < buf.len() && !buf.is_char_boundary(cut) {
            cut += 1;
        }
        buf.drain(..cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_and_osc() {
        let mut f = AnsiFilter::default();
        let mut out = String::new();
        f.feed(b"\x1b[31mred\x1b[0m and \x1b]0;title\x07ok", &mut out);
        assert_eq!(out, "red and ok");
    }

    #[test]
    fn handles_split_csi() {
        let mut f = AnsiFilter::default();
        let mut out = String::new();
        f.feed(b"\x1b[3", &mut out); // sequence split across reads
        f.feed(b"1mx\x1b[0m", &mut out);
        assert_eq!(out, "x");
    }

    #[test]
    fn strips_osc_split_at_st() {
        let mut f = AnsiFilter::default();
        let mut out = String::new();
        f.feed(b"\x1b]0;my", &mut out);
        f.feed(b"title\x1b\\after", &mut out);
        assert_eq!(out, "after");
    }

    #[test]
    fn collapses_crlf_and_maps_cr() {
        let mut f = AnsiFilter::default();
        let mut out = String::new();
        f.feed(b"line1\r\nline2\rdone", &mut out);
        assert_eq!(out, "line1\nline2\ndone");
    }

    #[test]
    fn utf8_split_across_chunks() {
        let mut f = AnsiFilter::default();
        let mut out = String::new();
        let bytes = "héllo 你好".as_bytes();
        for (i, _) in bytes.iter().enumerate() {
            f.feed(&bytes[i..=i], &mut out); // one byte per read: worst-case splits
        }
        assert_eq!(out, "héllo 你好");
    }

    #[test]
    fn output_cap_trims_front() {
        let mut f = AnsiFilter::default();
        let mut buf = String::new();
        for _ in 0..OUTPUT_CAP {
            append_output(&mut buf, b"ab", &mut f);
        }
        assert!(buf.len() <= OUTPUT_CAP);
        assert!(buf.len() >= OUTPUT_CAP / 2);
    }
}
