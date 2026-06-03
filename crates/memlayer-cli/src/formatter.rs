//! Output formatting: text/json/yaml dispatch with TTY-aware default and
//! `NO_COLOR` / `CLICOLOR_FORCE` handling.
//!
//! Spec sections: FR1.2, FR1.4, §11.2.
//!
//! Every RPC response type implements [`Render`] with a renderer per format.
//! The CLI calls [`Formatter::resolve`] once at startup with the parsed
//! `--output` flag and the current TTY status, then passes the resulting
//! enum into each subcommand handler.

use std::io::{self, Write};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formatter {
    Text,
    Json,
    Yaml,
}

impl Formatter {
    /// Pick the output format. An explicit `--output` always wins (FR1.2).
    /// Otherwise the default is `text` on a TTY, `json` when piped (SC-4/SC-5).
    pub fn resolve(explicit: Option<Formatter>, stdout_is_tty: bool) -> Formatter {
        match explicit {
            Some(f) => f,
            None if stdout_is_tty => Formatter::Text,
            None => Formatter::Json,
        }
    }
}

/// Renderer trait implemented by every RPC response type.
///
/// `render_json` and `render_yaml` have default implementations that delegate
/// to `serde`. Implementations override `render_text` to produce a
/// human-readable table or summary.
pub trait Render: Serialize {
    fn render_text(&self, w: &mut dyn Write) -> io::Result<()>;

    fn render_json(&self, w: &mut dyn Write) -> io::Result<()> {
        serde_json::to_writer_pretty(&mut *w, self).map_err(io::Error::other)?;
        writeln!(w)
    }

    fn render_yaml(&self, w: &mut dyn Write) -> io::Result<()> {
        serde_yaml::to_writer(&mut *w, self).map_err(io::Error::other)
    }

    fn render(&self, fmt: Formatter, w: &mut dyn Write) -> io::Result<()> {
        match fmt {
            Formatter::Text => self.render_text(w),
            Formatter::Json => self.render_json(w),
            Formatter::Yaml => self.render_yaml(w),
        }
    }
}

/// Decide whether ANSI color is permitted on stdout.
///
/// Precedence (highest first, per FR1.4):
/// 1. `--no-color` flag → false
/// 2. `NO_COLOR` env var (any value) → false
/// 3. `CLICOLOR_FORCE` env var (any value) → true
/// 4. stdout is a TTY → true
/// 5. otherwise → false
pub fn use_color(no_color_flag: bool, stdout_is_tty: bool) -> bool {
    if no_color_flag {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some() {
        return true;
    }
    stdout_is_tty
}

/// Strip ANSI Control Sequence Introducer escapes from `s`.
///
/// Used as a defense-in-depth pass when `use_color` returned false: even if
/// upstream code embedded color codes (e.g. via a third-party library), this
/// scrubs them before bytes reach the terminal. Handles CSI (`ESC [ … final`)
/// and OSC (`ESC ] … BEL`) sequences; unknown two-byte escapes are dropped
/// conservatively.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c2 in chars.by_ref() {
                    // CSI final byte is 0x40..=0x7E
                    let v = c2 as u32;
                    if (0x40..=0x7E).contains(&v) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2 == '\x07' {
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piped_default_is_json() {
        // No --output, stdout is not a TTY → JSON (SC-4).
        assert_eq!(Formatter::resolve(None, false), Formatter::Json);
    }

    #[test]
    fn tty_default_is_text() {
        // No --output, stdout is a TTY → human-readable text (SC-5).
        assert_eq!(Formatter::resolve(None, true), Formatter::Text);
    }

    #[test]
    fn explicit_output_wins_regardless_of_tty() {
        assert_eq!(Formatter::resolve(Some(Formatter::Yaml), true), Formatter::Yaml);
        assert_eq!(Formatter::resolve(Some(Formatter::Json), true), Formatter::Json);
        assert_eq!(Formatter::resolve(Some(Formatter::Text), false), Formatter::Text);
    }

    #[test]
    fn no_color_strips_ansi() {
        // Defense-in-depth: even if upstream code injects ANSI, strip_ansi
        // ensures the rendered string is plain text when --no-color or
        // NO_COLOR=1 (SC-6).
        let colored = "\x1b[31mhello\x1b[0m world\x1b[1mbold\x1b[0m";
        assert_eq!(strip_ansi(colored), "hello worldbold");
    }

    #[test]
    fn strip_ansi_passthrough_for_plain_text() {
        let plain = "no escapes here\nline2\ttab";
        assert_eq!(strip_ansi(plain), plain);
    }

    #[test]
    fn use_color_respects_no_color_flag() {
        // --no-color always wins, even on a TTY.
        assert!(!use_color(true, true));
    }

    #[test]
    fn use_color_returns_false_when_not_tty() {
        // Make sure neither NO_COLOR nor CLICOLOR_FORCE is in env to avoid
        // flakes; we cannot remove env vars set by the test runner reliably,
        // so we only assert the easy case (no flag, not a TTY).
        if std::env::var_os("CLICOLOR_FORCE").is_none() {
            assert!(!use_color(false, false));
        }
    }
}
