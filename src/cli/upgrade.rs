use super::*;
use crate::update::{InstallOutcome, UpdateClient};

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Only check the target release; do not download or install it.
    #[arg(long)]
    pub check: bool,
    /// Select a specific GitHub Release tag.
    #[arg(long)]
    pub version: Option<String>,
}

pub(super) async fn run(args: UpdateArgs, paths: &AppPaths) -> anyhow::Result<i32> {
    let config = match Config::load_network(&paths.config_file) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Configuration unavailable ({error}); using default network settings.");
            // An empty isolated directory preserves network environment overrides
            // without accidentally reading another user configuration file.
            let empty = tempfile::tempdir()?;
            Config::load_network(empty.path().join("config.toml"))?
        }
    };
    let (http, _, _, _) = client(&config, None, None)?;
    let updater = UpdateClient::new(http);
    let plan = updater
        .check(env!("CARGO_PKG_VERSION"), args.version.as_deref())
        .await?;
    println!("Current: {}\nTarget: {}", plan.current, plan.target);
    if !plan.notes.trim().is_empty() {
        println!("\n{}", plan.notes);
    }
    if args.check {
        return Ok(EXIT_OK);
    }
    if !plan.needs_install {
        println!("Already up to date.");
        return Ok(EXIT_OK);
    }
    print_outcome(updater.install(&plan, &paths.config_dir).await?)
}

pub(super) fn print_outcome(outcome: InstallOutcome) -> anyhow::Result<i32> {
    match outcome {
        InstallOutcome::Installed {
            version,
            backup,
            notices,
        } => {
            println!(
                "Updated to {version}. Previous binary: {}",
                backup.display()
            );
            for notice in notices {
                println!("{notice}");
            }
            Ok(EXIT_OK)
        }
        InstallOutcome::Scheduled { version } => {
            println!(
                "Update to {version} scheduled; replacement completes after this process exits."
            );
            Ok(EXIT_OK)
        }
        InstallOutcome::InstalledMigrationFailed {
            version,
            backup,
            error,
        } => {
            eprintln!(
                "Installed {version}, but migration did not complete: {error}\nPrevious binary: {}",
                backup.display()
            );
            Ok(EXIT_INIT)
        }
    }
}
