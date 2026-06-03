//! `logs` subcommand handlers (FR11, SC-25).
//!
//! `logs --lines N` prints the last N lines of `~/.memlayer/daemon.log`.
//! `logs -f` follows the file, streaming new lines until SIGINT (EC-10:
//! works even when the daemon is not running — we just tail the file).

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::cli::LogsArgs;
use crate::exit;

pub async fn dispatch(args: LogsArgs) -> ExitCode {
    let path = memlayer_core::paths::log_path();
    if !path.exists() {
        eprintln!(
            "memlayer: log file does not exist at {}; the daemon may not have started yet",
            path.display()
        );
        return ExitCode::from(exit::GENERAL);
    }
    if let Err(e) = print_tail(&path, args.lines) {
        eprintln!("memlayer: read {}: {e}", path.display());
        return ExitCode::from(exit::GENERAL);
    }
    if args.follow {
        if let Err(e) = follow(&path).await {
            eprintln!("memlayer: follow {}: {e}", path.display());
            return ExitCode::from(exit::GENERAL);
        }
    }
    ExitCode::SUCCESS
}

/// Print the last `n` lines of `path` to stdout. SC-25.
fn print_tail(path: &Path, n: usize) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let lines = tail_lines(reader, n)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    for line in lines {
        writeln!(handle, "{line}")?;
    }
    handle.flush()
}

/// Pure helper: collect all lines from `reader` and return the last `n`.
/// Used directly by the skeleton test `lines_flag_caps_output`.
pub fn tail_lines<R: Read>(reader: R, n: usize) -> io::Result<Vec<String>> {
    let buf = BufReader::new(reader);
    let all: Result<Vec<String>, io::Error> = buf.lines().collect();
    let all = all?;
    if all.len() <= n {
        Ok(all)
    } else {
        Ok(all[all.len() - n..].to_vec())
    }
}

/// Stream new bytes appended to `path` until SIGINT.
async fn follow(path: &Path) -> io::Result<()> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::End(0))?;
    let mut buf = [0u8; 8192];
    let stdout = io::stdout();
    loop {
        let n = file.read(&mut buf)?;
        if n > 0 {
            let mut handle = stdout.lock();
            handle.write_all(&buf[..n])?;
            handle.flush()?;
            continue;
        }
        // No new data — wait briefly, but bail out on SIGINT.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = tokio::signal::ctrl_c() => {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn lines_flag_caps_output() {
        // 20 lines of input, --lines 5 → last 5 returned (line-15..line-19).
        let body: String = (0..20).map(|i| format!("line-{i}\n")).collect();
        let r = Cursor::new(body.into_bytes());
        let lines = tail_lines(r, 5).unwrap();
        assert_eq!(lines.len(), 5, "expected 5 lines, got {}", lines.len());
        assert_eq!(lines.first().unwrap(), "line-15");
        assert_eq!(lines.last().unwrap(), "line-19");
    }

    #[test]
    fn tail_lines_returns_all_when_under_cap() {
        let body = "a\nb\nc\n";
        let r = Cursor::new(body.as_bytes());
        let lines = tail_lines(r, 100).unwrap();
        assert_eq!(lines, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn tail_lines_handles_empty_input() {
        let r = Cursor::new(&b""[..]);
        let lines = tail_lines(r, 10).unwrap();
        assert!(lines.is_empty());
    }
}
