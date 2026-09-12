//! `memlayer mem export|import` — portable `.mem` archives.

#![allow(clippy::result_large_err)]

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use memlayer_proto as p;
use memlayer_sync::mem_archive;

use crate::cli::{MemExportArgs, MemImportArgs, MemVerb};
use crate::cmd_obs::Client;
use crate::exit;
use crate::formatter::Formatter;

pub const HINT_UNENCRYPTED: &str = "\
note: archive is not seed-encrypted (obfuscated only).\n\
      encrypt with a seed phrase (same phrase required on import):\n\
        memlayer mem export --out FILE.mem --seed-file ./phrase.txt\n\
        memlayer mem export --out FILE.mem --seed-phrase 'your phrase here'\n\
      keep the phrase; it cannot be recovered.";

pub const HINT_ENCRYPTED: &str = "\
note: archive is seed-encrypted. import needs the same --seed-file or --seed-phrase.\n\
      if you lose the phrase, this file cannot be opened.";

pub const HINT_IMPORT_ENCRYPTED: &str = "\
note: imported a seed-encrypted archive.\n\
      future imports of this file need the same --seed-file or --seed-phrase.";

pub const HINT_IMPORT_UNENCRYPTED: &str = "\
note: archive is not seed-encrypted (obfuscated only).\n\
      if you expected encryption, re-export with --seed-file or --seed-phrase.";

pub const ERR_SEED_REQUIRED: &str = "\
error: this archive is seed-encrypted and cannot be imported without the seed phrase.
       pass the same phrase used at export:
         memlayer mem import FILE.mem --seed-file ./phrase.txt
         memlayer mem import FILE.mem --seed-phrase 'your phrase here'";

pub async fn dispatch(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    verb: MemVerb,
) -> ExitCode {
    let result = match verb {
        MemVerb::Export(a) => export(client, project_name, fmt, a).await,
        MemVerb::Import(a) => import(client, project_name, fmt, a).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(VerbErr::Usage(msg)) => {
            eprintln!("{msg}");
            ExitCode::from(exit::USAGE)
        }
        Err(VerbErr::Io(e)) => {
            eprintln!("memlayer: i/o error: {e}");
            ExitCode::from(exit::GENERAL)
        }
        Err(VerbErr::Status(s)) => {
            let msg = s.message();
            if msg.contains("cannot be imported without the seed phrase") {
                eprintln!("{}", ERR_SEED_REQUIRED.replace("FILE.mem", "<file>.mem"));
            } else {
                eprintln!("memlayer: {msg}");
            }
            ExitCode::from(exit::from_status(s.code()))
        }
    }
}

enum VerbErr {
    Usage(String),
    Io(io::Error),
    Status(tonic::Status),
}

async fn export(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: MemExportArgs,
) -> Result<(), VerbErr> {
    let out = a.out;
    if !out.to_string_lossy().ends_with(".mem") {
        return Err(VerbErr::Usage(
            "error: export path must end with .mem".into(),
        ));
    }
    let used_seed = a.seed_phrase.is_some() || a.seed_file.is_some();
    let seed_phrase = load_seed(a.seed_phrase, a.seed_file.as_deref())?;
    let project = a.project.unwrap_or_else(|| project_name.to_string());
    let req = p::ExportMemRequest {
        project_name: project,
        file: out.to_string_lossy().into_owned(),
        seed_phrase,
    };
    let resp = client.export_mem(req).await.map_err(VerbErr::Status)?.into_inner();
    write_export(&resp, fmt, used_seed).map_err(VerbErr::Io)?;
    if used_seed {
        eprintln!("{HINT_ENCRYPTED}");
    } else {
        eprintln!("{HINT_UNENCRYPTED}");
    }
    Ok(())
}

async fn import(
    client: &mut Client,
    project_name: &str,
    fmt: Formatter,
    a: MemImportArgs,
) -> Result<(), VerbErr> {
    let file = a.file;
    if !file.to_string_lossy().ends_with(".mem") {
        return Err(VerbErr::Usage(
            "error: import path must end with .mem".into(),
        ));
    }
    let bytes = std::fs::read(&file).map_err(VerbErr::Io)?;
    let encrypted = memlayer_archive_encrypted(&bytes, &file)?;
    let seed_phrase = load_seed(a.seed_phrase, a.seed_file.as_deref())?;
    if encrypted && seed_phrase.is_none() {
        let path = file.display().to_string();
        if fmt == Formatter::Json {
            let v = serde_json::json!({
                "error": "seed_required",
                "message": "this archive is seed-encrypted and cannot be imported without the seed phrase",
                "seed_encrypted": true,
            });
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        return Err(VerbErr::Usage(ERR_SEED_REQUIRED.replace("FILE.mem", &path)));
    }
    let project = a.project.unwrap_or_else(|| project_name.to_string());
    let req = p::ImportMemRequest {
        project_name: project,
        file: file.to_string_lossy().into_owned(),
        mode: a.mode,
        seed_phrase,
    };
    let resp = client.import_mem(req).await.map_err(VerbErr::Status)?.into_inner();
    write_import(&resp, fmt).map_err(VerbErr::Io)?;
    if resp.seed_encrypted {
        eprintln!("{HINT_IMPORT_ENCRYPTED}");
    } else {
        eprintln!("{HINT_IMPORT_UNENCRYPTED}");
        if resp.seed_ignored {
            eprintln!(
                "warn: seed was ignored because the archive is not seed-encrypted."
            );
        }
    }
    Ok(())
}

fn memlayer_archive_encrypted(bytes: &[u8], path: &Path) -> Result<bool, VerbErr> {
    mem_archive::is_encrypted(bytes).map_err(|e| match e {
        memlayer_sync::SyncError::NotArchive => VerbErr::Usage(format!(
            "error: {} is not a memlayer archive",
            path.display()
        )),
        other => VerbErr::Usage(format!("error: {other}")),
    })
}

fn load_seed(
    seed_phrase: Option<String>,
    seed_file: Option<&Path>,
) -> Result<Option<String>, VerbErr> {
    if let Some(p) = seed_file {
        let s = std::fs::read_to_string(p).map_err(VerbErr::Io)?;
        Ok(Some(s))
    } else {
        Ok(seed_phrase)
    }
}

fn write_export(resp: &p::ExportMemResponse, fmt: Formatter, seed_encrypted: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    match fmt {
        Formatter::Text => {
            writeln!(
                handle,
                "wrote {} ({} observations, {} sessions, {} bytes)",
                resp.file, resp.observations, resp.sessions, resp.bytes
            )?;
        }
        Formatter::Json | Formatter::Yaml => {
            let hint = if seed_encrypted {
                HINT_ENCRYPTED.lines().next().unwrap_or("")
            } else {
                HINT_UNENCRYPTED.lines().next().unwrap_or("")
            };
            let v = serde_json::json!({
                "file": resp.file,
                "observations": resp.observations,
                "sessions": resp.sessions,
                "prompts": resp.prompts,
                "facts": resp.facts,
                "relations": resp.relations,
                "bytes": resp.bytes,
                "seed_encrypted": seed_encrypted,
                "hint": hint,
            });
            if fmt == Formatter::Yaml {
                writeln!(handle, "{}", serde_yaml::to_string(&v).unwrap_or_default())?;
            } else {
                serde_json::to_writer_pretty(&mut handle, &v)?;
                writeln!(handle)?;
            }
        }
    }
    handle.flush()
}

fn write_import(resp: &p::ImportMemResponse, fmt: Formatter) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    match fmt {
        Formatter::Text => {
            writeln!(
                handle,
                "imported {} observations, {} sessions, {} prompts, {} facts ({} skipped)",
                resp.observations_imported,
                resp.sessions_imported,
                resp.prompts_imported,
                resp.facts_imported,
                resp.skipped
            )?;
        }
        Formatter::Json | Formatter::Yaml => {
            let hint = if resp.seed_encrypted {
                HINT_IMPORT_ENCRYPTED.lines().next().unwrap_or("")
            } else {
                HINT_IMPORT_UNENCRYPTED.lines().next().unwrap_or("")
            };
            let v = serde_json::json!({
                "observations_imported": resp.observations_imported,
                "sessions_imported": resp.sessions_imported,
                "prompts_imported": resp.prompts_imported,
                "facts_imported": resp.facts_imported,
                "relations_imported": resp.relations_imported,
                "skipped": resp.skipped,
                "seed_encrypted": resp.seed_encrypted,
                "seed_ignored": resp.seed_ignored,
                "hint": hint,
            });
            if fmt == Formatter::Yaml {
                writeln!(handle, "{}", serde_yaml::to_string(&v).unwrap_or_default())?;
            } else {
                serde_json::to_writer_pretty(&mut handle, &v)?;
                writeln!(handle)?;
            }
        }
    }
    handle.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unencrypted_hint_mentions_seed_flags() {
        assert!(HINT_UNENCRYPTED.contains("--seed-file"));
        assert!(HINT_UNENCRYPTED.contains("--seed-phrase"));
        assert!(HINT_UNENCRYPTED.contains("not seed-encrypted"));
    }

    #[test]
    fn import_without_seed_error_is_actionable() {
        assert!(ERR_SEED_REQUIRED.contains("seed-encrypted"));
        assert!(ERR_SEED_REQUIRED.contains("cannot be imported without the seed phrase"));
        assert!(ERR_SEED_REQUIRED.contains("--seed-file"));
        assert!(ERR_SEED_REQUIRED.contains("--seed-phrase"));
        assert!(!ERR_SEED_REQUIRED.to_lowercase().contains("aead"));
        assert!(!ERR_SEED_REQUIRED.to_lowercase().contains("zstd"));
    }
}
