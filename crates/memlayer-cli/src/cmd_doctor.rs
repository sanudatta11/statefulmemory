//! CLI command handler for `memlayer doctor` (health audit & auto-repair).

use crate::cli::DoctorArgs;
use memlayer_client::MemlayerClient;
use memlayer_proto::DoctorRequest;
use memlayer_storage::doctor::DoctorFinding;
use tonic::transport::Channel;

pub async fn run(
    client: Option<&mut MemlayerClient<Channel>>,
    project_name: &str,
    args: DoctorArgs,
    output_json: bool,
) -> anyhow::Result<()> {
    let findings: Vec<DoctorFinding> = if let Some(c) = client {
        let resp = c
            .doctor(DoctorRequest {
                project_name: Some(project_name.to_string()),
                auto_repair: args.repair,
            })
            .await?;
        resp.into_inner()
            .findings
            .into_iter()
            .map(|f| DoctorFinding {
                code: f.code,
                severity: f.severity,
                message: f.message,
                remedy: f.remedy,
            })
            .collect()
    } else {
        // Fallback: direct local database audit if daemon is offline
        let data_dir = memlayer_core::paths::data_dir();
        let db_path = data_dir.join("projects").join(project_name).join("memlayer.db");
        if !db_path.exists() {
            let finding = DoctorFinding {
                code: "DB_MISSING".into(),
                severity: "warn".into(),
                message: format!("Database file does not exist at {}", db_path.display()),
                remedy: Some("Store an observation or run `memlayer obs save` to initialize project".into()),
            };
            vec![finding]
        } else {
            let conn = if args.repair {
                memlayer_storage::db::open_write(&db_path)?
            } else {
                memlayer_storage::db::open_read(&db_path)?
            };
            memlayer_storage::doctor::audit_and_repair(&conn, args.repair)?
        }
    };

    if output_json {
        let json = serde_json::to_string_pretty(&findings)?;
        println!("{json}");
        return Ok(());
    }

    // Render formatted human-readable table
    println!("{:<8} {:<18} {:<45} {}", "STATUS", "CODE", "MESSAGE", "REMEDY");
    println!("{}", "-".repeat(90));

    let mut warn_count = 0;
    let mut err_count = 0;
    let mut ok_count = 0;

    for f in &findings {
        let status_tag = match f.severity.as_str() {
            "info" => {
                ok_count += 1;
                "[OK]"
            }
            "warn" => {
                warn_count += 1;
                "[WARN]"
            }
            "error" => {
                err_count += 1;
                "[ERR]"
            }
            _ => "[INFO]",
        };
        let remedy_str = f.remedy.as_deref().unwrap_or("-");
        println!(
            "{:<8} {:<18} {:<45} {}",
            status_tag, f.code, f.message, remedy_str
        );
    }

    println!("{}", "-".repeat(90));
    if args.repair {
        println!("Doctor auto-repair completed. Checks: {ok_count} passed, {warn_count} warnings, {err_count} errors.");
    } else {
        println!("Doctor health check finished. Checks: {ok_count} passed, {warn_count} warnings, {err_count} errors.");
    }

    Ok(())
}
