//! `version` subcommand (FR11, SC-24).
//!
//! Prints `memlayer X.Y.Z (commit <sha>, built <date>)`. The git SHA and
//! build timestamp come from `build.rs`-emitted env vars; the version
//! number is the crate's `CARGO_PKG_VERSION`.

use std::process::ExitCode;

use chrono::{TimeZone, Utc};

/// Build the version string. Used by both the runtime printout and the
/// SC-24 regex test.
pub fn version_string() -> String {
    let sha: &str = env!("MEMLAYER_GIT_SHA");
    let ts: i64 = env!("MEMLAYER_BUILD_TS").parse().unwrap_or(0);
    let date = Utc
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "memlayer {} (commit {sha}, built {date})",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn run() -> ExitCode {
    println!("{}", version_string());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_format_matches_regex() {
        // SC-24: `memlayer [0-9]+\.[0-9]+\.[0-9]+ \(commit [0-9a-f]+`. We
        // implement the regex check by hand to avoid a `regex` dev-dep.
        let s = version_string();
        assert!(s.starts_with("memlayer "), "missing 'memlayer ' prefix: {s}");
        assert!(s.contains(" (commit "), "missing ' (commit ': {s}");
        assert!(s.contains(", built "), "missing ', built ': {s}");
        assert!(s.ends_with(')'), "missing closing paren: {s}");

        // Pull out the version segment between "memlayer " and " (commit ".
        let after_prefix = s.strip_prefix("memlayer ").unwrap();
        let version = after_prefix.split(' ').next().unwrap();
        let nums: Vec<&str> = version.split('.').collect();
        assert_eq!(nums.len(), 3, "version must be X.Y.Z, got {version}");
        for n in &nums {
            assert!(
                !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()),
                "non-numeric component in version: {n}"
            );
        }

        // SHA must be hex (or the literal "unknown" when `git` isn't
        // available in the build env). Either is acceptable for SC-24.
        let after_commit = s.split("(commit ").nth(1).unwrap();
        let sha = after_commit.split(',').next().unwrap();
        let is_hex = !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit());
        let is_unknown = sha == "unknown";
        assert!(
            is_hex || is_unknown,
            "commit field is neither hex nor 'unknown': {sha}"
        );
    }

    #[test]
    fn version_string_contains_pkg_version() {
        let s = version_string();
        assert!(s.contains(env!("CARGO_PKG_VERSION")));
    }
}
