use super::*;
use crate::channel::{BUILTIN_CATALOG, ChangeSummary, ChannelStore, MAX_ENABLED_CHANNELS};

#[derive(Debug, Args)]
pub struct ChannelArgs {
    #[command(subcommand)]
    pub command: ChannelCommand,
}

#[derive(Debug, Subcommand)]
pub enum ChannelCommand {
    /// List saved channels and their enabled status.
    List,
    /// Add public channel names or URLs (enabled unless --disabled).
    Add {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
        #[arg(long)]
        disabled: bool,
    },
    Remove {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    Enable {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    Disable {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    /// Show the offline candidate catalog shipped with this version.
    Catalog,
    /// Import missing channels as disabled, preserving existing choices.
    Import {
        #[arg(long, conflicts_with = "source", required_unless_present = "source")]
        builtin: bool,
        #[arg(required_unless_present = "builtin")]
        source: Option<String>,
    },
    Path,
}

pub async fn run(args: ChannelArgs, paths: &AppPaths) -> anyhow::Result<i32> {
    let store = ChannelStore::new(&paths.channels_file);
    // Notices do not prevent local repair when unrelated configuration is broken.
    if let Ok(notices) = crate::migration::notices(&paths.config_dir) {
        for notice in notices {
            eprintln!("{}", notice.message);
        }
    }
    let change = match args.command {
        ChannelCommand::List => {
            let list = store.load()?;
            println!("CHANNEL                          ENABLED");
            for channel in &list.channels {
                println!(
                    "{:<32} {}",
                    channel.name,
                    if channel.enabled { "yes" } else { "no" }
                );
            }
            println!(
                "\nEnabled: {}/{} · Total: {}",
                list.enabled_names().len(),
                MAX_ENABLED_CHANNELS,
                list.channels.len()
            );
            return Ok(EXIT_OK);
        }
        ChannelCommand::Catalog => {
            print!("{BUILTIN_CATALOG}");
            return Ok(EXIT_OK);
        }
        ChannelCommand::Path => {
            println!("{}", store.path().display());
            return Ok(EXIT_OK);
        }
        ChannelCommand::Add { names, disabled } => store.add(&names, !disabled)?,
        ChannelCommand::Remove { names } => store.remove(&names)?,
        ChannelCommand::Enable { names } => store.set_enabled(&names, true)?,
        ChannelCommand::Disable { names } => store.set_enabled(&names, false)?,
        ChannelCommand::Import { builtin, source } => {
            let result = if builtin {
                store.import_text(BUILTIN_CATALOG)?
            } else {
                let source = source.expect("clap requires import source");
                // Only remote imports need network configuration.
                let config = if source.contains("://") {
                    Config::load_network(&paths.config_file)?
                } else {
                    Config::default()
                };
                let (http, _, timeout, _) = client(&config, None, None)?;
                store.import_source(&source, &http, timeout).await?
            };
            println!(
                "Added: {} · Already present: {}\nNew channels are disabled.",
                result.added, result.existing
            );
            return Ok(EXIT_OK);
        }
    };
    print_change(&change);
    Ok(EXIT_OK)
}

fn print_change(change: &ChangeSummary) {
    println!(
        "Added: {} · Already present: {} · Changed: {} · Removed: {}",
        change.added, change.existing, change.changed, change.removed
    );
    for name in &change.not_found {
        eprintln!("Channel not found: {name}");
    }
}
