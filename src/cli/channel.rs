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
    /// Add searchable public Telegram channels (enabled unless --disabled).
    ///
    /// Supported channels have a public username and a message preview at
    /// https://t.me/s/<name>. Bots, private invite links, and names without a
    /// public message page are not supported.
    Add {
        /// Public channel usernames, @usernames, or https://t.me/ URLs.
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
        /// Save new channels without enabling them for searches.
        #[arg(long)]
        disabled: bool,
        /// Skip checking that each name has a searchable public message page.
        #[arg(long)]
        no_validate: bool,
    },
    /// Remove saved channels from the custom channel list.
    Remove {
        /// Channel usernames, @usernames, or https://t.me/ URLs to remove.
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    /// Enable saved channels for searches (up to 128 enabled channels).
    Enable {
        /// Saved channel usernames, @usernames, or https://t.me/ URLs to enable.
        #[arg(required_unless_present = "all", conflicts_with = "all", num_args = 1..)]
        names: Vec<String>,
        /// Enable every saved channel; reject the whole operation if it exceeds 128.
        #[arg(long)]
        all: bool,
    },
    /// Disable saved channels without removing them.
    Disable {
        /// Saved channel usernames, @usernames, or https://t.me/ URLs to disable.
        #[arg(required_unless_present = "all", conflicts_with = "all", num_args = 1..)]
        names: Vec<String>,
        /// Disable every saved channel without removing any entries.
        #[arg(long)]
        all: bool,
    },
    /// Show the offline candidate catalog shipped with this version.
    Catalog,
    /// Import missing channels as enabled unless --disable; preserve existing choices.
    Import {
        /// Import the built-in catalog to expand your available Telegram sources.
        #[arg(long, conflicts_with = "source", required_unless_present = "source")]
        builtin: bool,
        /// Local file or HTTP(S) URL with one channel per line; # starts a comment.
        #[arg(required_unless_present = "builtin")]
        source: Option<String>,
        /// Save newly imported channels as disabled.
        #[arg(long)]
        disable: bool,
    },
    /// Print the path to the standalone channels.toml configuration file.
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
        ChannelCommand::Add {
            names,
            disabled,
            no_validate,
        } => {
            let names = if no_validate {
                crate::channel::normalize_channels(&names)?
            } else {
                let config = Config::load_network(&paths.config_file)?;
                let (http, _, timeout, _) = client(&config, None, None)?;
                crate::channel::ChannelValidator::new(http, timeout)
                    .validate(&names)
                    .await?
            };
            store.add(&names, !disabled)?
        }
        ChannelCommand::Remove { names } => store.remove(&names)?,
        ChannelCommand::Enable { all: true, .. } => store.set_all_enabled(true)?,
        ChannelCommand::Disable { all: true, .. } => store.set_all_enabled(false)?,
        ChannelCommand::Enable { names, all: false } => store.set_enabled(&names, true)?,
        ChannelCommand::Disable { names, all: false } => store.set_enabled(&names, false)?,
        ChannelCommand::Import {
            builtin,
            source,
            disable,
        } => {
            let result = if builtin {
                store.import_text(BUILTIN_CATALOG, !disable)?
            } else {
                let source = source.expect("clap requires import source");
                // Only remote imports need network configuration.
                let config = if source.contains("://") {
                    Config::load_network(&paths.config_file)?
                } else {
                    Config::default()
                };
                let (http, _, timeout, _) = client(&config, None, None)?;
                store
                    .import_source(&source, &http, timeout, !disable)
                    .await?
            };
            println!(
                "Added: {} · Already present: {}\nNew channels are {}. Existing channels keep their saved status.",
                result.added,
                result.existing,
                if disable { "disabled" } else { "enabled" }
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
