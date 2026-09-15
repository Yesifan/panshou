use std::{
    collections::HashSet,
    ffi::OsString,
    io::{self, BufRead, Read, Write},
    path::Path,
    process::Command as ProcessCommand,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};

mod channel;
mod streaming;
mod upgrade;

use crate::{
    check::{CheckCache, CheckEngine, CheckItem, CheckOptions, CheckState, proxy_scope},
    config::{AppPaths, Config, ConfigOverrides},
    core::CloudType,
    http::{ClientOptions, HttpClientFactory, ProxyUrl, RedirectPolicy},
    output::{OutputFormat, stdout, write_checks},
    providers::{
        Provider, SearchContext, builtin_stateless_providers,
        gying::{Endpoints as GyingEndpoints, GyingProfile, GyingProvider},
        panlian::{Endpoints as PanlianEndpoints, PanlianProfile, PanlianProvider},
        qqpd::{QqQrStatus, QqpdProfile, QqpdProvider},
        weibo::{QrStatus, WeiboProfile, WeiboProvider},
    },
    search::{SearchEngine, SearchOptions},
    state::StateStore,
};

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_SEARCH_FAILED: i32 = 3;
pub const EXIT_AUTH_REQUIRED: i32 = 4;
pub const EXIT_INIT: i32 = 5;
pub const EXIT_INVALID_LINK: i32 = 10;
pub const EXIT_DEADLINE: i32 = 124;
pub const EXIT_INTERRUPTED: i32 = 130;

const SEARCH_GUIDANCE: &str = "Resource names may be inconsistent across sources. Use short, focused keywords; longer queries may reduce search quality.\nResults stream as sources finish. --timeout excludes queueing; --all-timeout includes queueing, but excludes initialization and link checking.";

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct UsageError(String);

fn usage(message: impl Into<String>) -> anyhow::Error {
    UsageError(message.into()).into()
}

pub fn error_exit_code(error: &anyhow::Error) -> i32 {
    if error.downcast_ref::<UsageError>().is_some()
        || error.downcast_ref::<crate::config::ConfigError>().is_some()
        || error.downcast_ref::<crate::core::ParseError>().is_some()
        || error
            .downcast_ref::<crate::channel::ChannelError>()
            .is_some()
    {
        EXIT_USAGE
    } else if matches!(
        error.downcast_ref::<crate::core::ProviderError>(),
        Some(crate::core::ProviderError::AuthRequired)
    ) {
        EXIT_AUTH_REQUIRED
    } else {
        EXIT_INIT
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "pansou",
    version,
    about = "Search and validate cloud-drive links",
    after_help = SEARCH_GUIDANCE
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search Telegram channels and providers, streaming results as they arrive.
    Search(SearchArgs),
    /// Validate cloud-drive links and report their availability.
    Check(CheckArgs),
    /// Manage search providers, login sessions, and profiles.
    Provider(ProviderArgs),
    /// Show the effective configuration or its file path.
    Config(ConfigArgs),
    /// Manage Telegram channels and import candidate channel lists.
    Channel(channel::ChannelArgs),
    /// Check GitHub Releases and install a verified update.
    Update(upgrade::UpdateArgs),
    #[command(name = "__update-preflight", hide = true)]
    UpdatePreflight { config_dir: std::path::PathBuf },
    #[command(name = "__update-finish", hide = true)]
    UpdateFinish { config_dir: std::path::PathBuf },
    #[command(name = "__update-replace", hide = true)]
    UpdateReplace {
        staged: std::path::PathBuf,
        target: std::path::PathBuf,
        config_dir: std::path::PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SourceSelection {
    All,
    Tg,
    Provider,
}

#[derive(Debug, Args)]
#[command(after_help = SEARCH_GUIDANCE)]
pub struct SearchArgs {
    /// Short keywords to search for.
    pub query: String,
    /// Select Telegram, providers, or both.
    #[arg(long, value_enum, default_value_t = SourceSelection::All)]
    pub source: SourceSelection,
    /// Select a provider; repeat to select multiple providers.
    #[arg(long = "provider")]
    pub providers: Vec<String>,
    /// Override saved Telegram channels for this search; repeat as needed.
    #[arg(long = "channel")]
    pub channels: Vec<String>,
    /// Keep only this cloud-drive type; repeat as needed.
    #[arg(long = "cloud")]
    pub clouds: Vec<CloudType>,
    /// Require an additional keyword in results; repeat as needed.
    #[arg(long = "include")]
    pub include: Vec<String>,
    /// Exclude results containing this keyword; repeat as needed.
    #[arg(long = "exclude")]
    pub exclude: Vec<String>,
    #[arg(long, help = "Shared source concurrency (default: 8)")]
    pub jobs: Option<usize>,
    #[arg(
        long,
        help = "Per-source timeout in seconds, excluding queueing (default: 30)"
    )]
    pub timeout: Option<u64>,
    #[arg(
        long,
        help = "Search deadline in seconds, including queueing (default: 600)"
    )]
    pub all_timeout: Option<u64>,
    /// Override the configured proxy URL.
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long, value_parser = parse_search_format, default_value = "table", help = "Streaming output: table or jsonl")]
    pub format: OutputFormat,
    /// Skip link validation and show all unchecked results.
    #[arg(long)]
    pub no_check: bool,
    /// Show detailed progress and diagnostics.
    #[arg(long, conflicts_with = "quiet")]
    pub verbose: bool,
    /// Suppress progress messages.
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Cloud-drive URLs to validate.
    pub urls: Vec<String>,
    /// Read URLs from standard input, one per line.
    #[arg(long)]
    pub stdin: bool,
    /// Override the detected cloud-drive type.
    #[arg(long = "type")]
    pub cloud_type: Option<String>,
    /// Share extraction code (not an account password).
    #[arg(long)]
    pub password: Option<String>,
    /// Maximum concurrent link checks.
    #[arg(long)]
    pub jobs: Option<usize>,
    /// Timeout per link check, in seconds.
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Override the configured proxy URL.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Recheck links instead of using cached results.
    #[arg(long)]
    pub refresh: bool,
    /// Disable reading and writing the check cache.
    #[arg(long)]
    pub no_cache: bool,
    /// Select the output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
    /// Exit with a nonzero status if any link is invalid.
    #[arg(long)]
    pub fail_invalid: bool,
}

#[derive(Debug, Args)]
pub struct ProviderArgs {
    #[command(subcommand)]
    pub command: ProviderCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// List available providers and their readiness.
    List,
    /// List saved profiles for a provider.
    Profiles {
        /// Provider name.
        name: String,
    },
    /// Authenticate a provider and save its session.
    Login(LoginArgs),
    /// Remove a saved provider session.
    Logout {
        /// Provider name.
        name: String,
        /// Saved profile to log out.
        #[arg(long, default_value = "main")]
        profile: String,
    },
    /// Show authentication status for a provider or profile.
    Status {
        /// Provider name.
        name: String,
        /// Limit status to one saved profile.
        #[arg(long)]
        profile: Option<String>,
    },
    /// Configure provider-specific sources and settings.
    Configure(ConfigureArgs),
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Provider name. See the possible values below for its login method.
    pub name: LoginProviderName,
    /// Profile name for the saved session.
    #[arg(long, default_value = "main")]
    pub profile: String,
    /// Account username, when required by the provider.
    #[arg(long)]
    pub username: Option<String>,
    /// Read the account password from standard input instead of prompting.
    #[arg(long)]
    pub password_stdin: bool,
    /// Remember credentials for providers that support automatic login.
    #[arg(long)]
    pub remember_credentials: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum LoginProviderName {
    /// QR code login; scan with QQ. No username or password options are needed.
    Qqpd,
    /// QR code login; scan with Weibo. No username or password options are needed.
    Weibo,
    /// Username/password login; requires --username. The password is prompted unless --password-stdin is used; --remember-credentials is optional.
    Gying,
    /// Username/password login; requires --username. The password is prompted unless --password-stdin is used; --remember-credentials is optional.
    Panlian,
}

impl LoginProviderName {
    const ALL: [Self; 4] = [Self::Qqpd, Self::Weibo, Self::Gying, Self::Panlian];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qqpd => "qqpd",
            Self::Weibo => "weibo",
            Self::Gying => "gying",
            Self::Panlian => "panlian",
        }
    }

    const fn supports_remembered_credentials(self) -> bool {
        matches!(self, Self::Gying | Self::Panlian)
    }

    const fn auth_label(self) -> &'static str {
        match self {
            Self::Qqpd | Self::Weibo => "qr",
            Self::Gying | Self::Panlian => "password",
        }
    }
}

#[derive(Debug, Args)]
pub struct ConfigureArgs {
    /// Provider name.
    pub name: String,
    /// Profile to configure.
    #[arg(long, default_value = "main")]
    pub profile: String,
    /// Comma-separated provider-specific channel identifiers.
    #[arg(long, value_delimiter = ',')]
    pub channels: Vec<String>,
    /// Provider-specific channel identifier; repeat as needed.
    #[arg(long = "channel")]
    pub channel: Vec<String>,
    /// Comma-separated provider-specific user identifiers.
    #[arg(long, value_delimiter = ',')]
    pub users: Vec<String>,
    /// Provider-specific user identifier; repeat as needed.
    #[arg(long = "user")]
    pub user: Vec<String>,
    /// Provider base URL.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Cloud-drive type to block for this provider; repeat as needed.
    #[arg(long = "blocked-cloud")]
    pub blocked_clouds: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective configuration.
    Show,
    /// Print the configuration file path.
    Path,
}

pub async fn run(cli: Cli) -> anyhow::Result<i32> {
    // Repair and update entry points must not depend on valid search configuration.
    match cli.command {
        Command::UpdatePreflight { config_dir } => {
            crate::migration::preflight(&config_dir)?;
            return Ok(EXIT_OK);
        }
        Command::UpdateFinish { config_dir } => {
            for notice in crate::migration::check(&config_dir)? {
                println!("{}", notice.message);
            }
            return Ok(EXIT_OK);
        }
        Command::UpdateReplace {
            staged,
            target,
            config_dir,
        } => {
            return upgrade::print_outcome(crate::update::replace_after_exit(
                &staged,
                &target,
                &config_dir,
            )?);
        }
        _ => {}
    }
    let paths = AppPaths::discover()?;
    if let Command::Channel(args) = cli.command {
        return channel::run(args, &paths).await;
    }
    if let Command::Update(args) = cli.command {
        return upgrade::run(args, &paths).await;
    }
    if matches!(
        cli.command,
        Command::Config(ConfigArgs {
            command: ConfigCommand::Path
        })
    ) {
        println!("{}", paths.config_file.display());
        return Ok(EXIT_OK);
    }
    let quiet = matches!(&cli.command, Command::Search(args) if args.quiet);
    for notice in crate::migration::notices(&paths.config_dir)? {
        if !quiet {
            eprintln!("{}", notice.message);
        }
    }
    let uses_channels = matches!(&cli.command, Command::Search(args) if args.source != SourceSelection::Provider && args.channels.is_empty() && std::env::var_os("PANSOU_CHANNELS").is_none() && std::env::var_os("CHANNELS").is_none());
    let mut config = if uses_channels {
        Config::load(&paths)?
    } else {
        Config::load_without_channels(&paths)?
    };
    match cli.command {
        Command::Search(args) => run_search(args, &paths, config).await,
        Command::Check(args) => run_check(args, &paths, config).await,
        Command::Provider(args) => run_provider(args, &paths, &mut config).await,
        Command::Config(args) => {
            match args.command {
                ConfigCommand::Show => println!("{}", toml::to_string_pretty(&config)?),
                ConfigCommand::Path => println!("{}", paths.config_file.display()),
            }
            Ok(EXIT_OK)
        }
        _ => unreachable!("maintenance commands handled before configuration loading"),
    }
}

fn parse_search_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "table" => Ok(OutputFormat::Table),
        "jsonl" => Ok(OutputFormat::Jsonl),
        "json" => Err("search JSON output was removed; use --format jsonl".into()),
        _ => Err("expected table or jsonl".into()),
    }
}

/// Read-only help: a broken configuration should never hide static usage.
pub fn help_defaults() -> String {
    let summary = (|| -> anyhow::Result<String> {
        let paths = AppPaths::discover()?;
        let config = if std::env::var_os("PANSOU_CHANNELS").is_some()
            || std::env::var_os("CHANNELS").is_some()
        {
            Config::load_without_channels(&paths)?
        } else {
            Config::load(&paths)?
        };
        let store = StateStore::from_paths(&paths)?;
        let mut providers = builtin_stateless_providers()
            .into_iter()
            .filter(|provider| {
                config
                    .search
                    .providers
                    .iter()
                    .any(|name| name == provider.meta().name)
            })
            .count();
        for name in LoginProviderName::ALL.map(LoginProviderName::as_str) {
            if stateful_ready(&store, name)? {
                providers += 1;
            }
        }
        let channels = crate::channel::validate_search_channels(&config.search.channels)?.len();
        let mut text = search_defaults_text(
            providers,
            channels,
            config.search.jobs,
            config.network.timeout_secs,
            config.search.all_timeout_secs,
        );
        // Inspect saved entries, not transient search overrides. Disabled entries
        // still count as imported; no import-history metadata is needed.
        if let Ok(saved) = crate::channel::ChannelStore::new(&paths.channels_file).load() {
            let catalog = crate::channel::parse_catalog(crate::channel::BUILTIN_CATALOG)?;
            if catalog
                .iter()
                .any(|name| !saved.channels.iter().any(|c| &c.name == name))
            {
                text.push_str("\nExpand Telegram sources with `pansou channel import --builtin`. Some built-in candidates are not in your saved list. New entries are enabled by default; use `--disable` to import them as disabled. Existing channels keep their saved status. Use `pansou channel enable --all` or `pansou channel disable --all` to change all saved channels.");
            }
        }
        Ok(text)
    })();
    match summary {
        Ok(text) => text,
        Err(error) => format!("Current search defaults: unavailable ({error})."),
    }
}

/// Whether dynamic search defaults belong with this top-level invocation.
///
/// `args` excludes the executable name. Subcommand help must stay static and
/// focused on that subcommand.
pub fn is_root_help_request(args: &[OsString]) -> bool {
    args.is_empty()
        || (args.len() == 1 && matches!(args[0].to_str(), Some("help" | "-h" | "--help")))
}

fn search_defaults_text(
    providers: usize,
    channels: usize,
    jobs: usize,
    timeout: u64,
    all_timeout: u64,
) -> String {
    let seconds = (providers + channels).div_ceil(jobs.max(1)) as u64;
    let seconds = seconds.saturating_mul(timeout).min(all_timeout);
    format!(
        "Current search defaults:\n  Providers: {providers}\n  Enabled channels: {channels}/{}\n  Concurrency: {jobs}\n  Timeout per source: {timeout}s\n  Overall search timeout: {all_timeout}s\n  Estimated maximum search wait: ~{seconds}s (~{:.1}m)\nResults stream as sources finish. Initialization and link checking may take additional time.",
        crate::channel::MAX_ENABLED_CHANNELS,
        seconds as f64 / 60.0
    )
}

fn client(
    config: &Config,
    proxy: Option<String>,
    timeout: Option<u64>,
) -> anyhow::Result<(reqwest::Client, Option<String>, Duration, ClientOptions)> {
    let proxy = proxy.or_else(|| config.network.proxy.clone());
    let timeout = Duration::from_secs(timeout.unwrap_or(config.network.timeout_secs));
    if timeout.is_zero() {
        return Err(usage("timeout must be greater than zero"));
    }
    let proxy_url = proxy
        .as_deref()
        .map(ProxyUrl::from_str)
        .transpose()
        .map_err(|error| usage(error.to_string()))?;
    let options = ClientOptions {
        proxy: proxy_url,
        timeout,
        ..ClientOptions::default()
    };
    Ok((
        HttpClientFactory::new().client(&options)?,
        proxy,
        timeout,
        options,
    ))
}

fn configured_stateful(
    store: &StateStore,
    config: &Config,
    options: &ClientOptions,
) -> anyhow::Result<Vec<Arc<dyn Provider>>> {
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
    if stateful_ready(store, "qqpd")? {
        providers.push(Arc::new(QqpdProvider::new(store.clone())));
    }
    if stateful_ready(store, "weibo")? {
        providers.push(Arc::new(WeiboProvider::new(store.clone())));
    }
    if stateful_ready(store, "gying")? {
        let endpoints = GyingEndpoints::parse(&config.providers.gying.base_url)?;
        providers.push(Arc::new(GyingProvider::load(
            store.clone(),
            endpoints,
            HttpClientFactory::new(),
            options.clone(),
        )?));
    }
    if stateful_ready(store, "panlian")? {
        let blocked = config
            .providers
            .panlian
            .blocked_clouds
            .iter()
            .map(|value| CloudType::from_str(value))
            .collect::<Result<Vec<_>, _>>()?;
        providers.push(Arc::new(PanlianProvider::load(
            store.clone(),
            PanlianEndpoints::default(),
            blocked,
        )?));
    }
    Ok(providers)
}

fn stateful_ready(store: &StateStore, name: &str) -> anyhow::Result<bool> {
    for profile in store.list_profiles(name)? {
        let ready = match name {
            "qqpd" => store
                .load::<QqpdProfile>(name, &profile)?
                .is_some_and(|state| {
                    state.is_ready(chrono::Utc::now()) && !state.channels.is_empty()
                }),
            "weibo" => store
                .load::<WeiboProfile>(name, &profile)?
                .is_some_and(|state| {
                    state.is_ready(chrono::Utc::now()) && !state.user_ids.is_empty()
                }),
            "gying" => store
                .load::<GyingProfile>(name, &profile)?
                .is_some_and(|state| state.status == "active" && !state.cookie.trim().is_empty()),
            "panlian" => store
                .load::<PanlianProfile>(name, &profile)?
                .is_some_and(|state| state.status == "active" && !state.cookie.trim().is_empty()),
            _ => false,
        };
        if ready {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn run_search(args: SearchArgs, paths: &AppPaths, mut config: Config) -> anyhow::Result<i32> {
    init_logging(args.verbose, args.quiet);
    config.apply_overrides(ConfigOverrides {
        proxy: args.proxy.clone(),
        timeout_secs: args.timeout,
        all_timeout_secs: args.all_timeout,
        search_jobs: args.jobs,
        channels: (!args.channels.is_empty()).then(|| args.channels.clone()),
        providers: (!args.providers.is_empty()).then(|| args.providers.clone()),
        check_jobs: None,
    })?;
    let (http, proxy, timeout, client_options) = client(&config, args.proxy, args.timeout)?;
    let providers = if args.source == SourceSelection::Tg {
        Vec::new()
    } else {
        let store = StateStore::from_paths(paths)?;
        let mut available = builtin_stateless_providers();
        available.extend(configured_stateful(&store, &config, &client_options)?);
        let explicit = !args.providers.is_empty();
        let selected_names: HashSet<_> = if !explicit {
            let mut names = config
                .search
                .providers
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            for name in LoginProviderName::ALL.map(LoginProviderName::as_str) {
                if !store.list_profiles(name)?.is_empty() {
                    names.insert(name);
                }
            }
            names
        } else {
            args.providers.iter().map(String::as_str).collect()
        };
        let known = available
            .iter()
            .map(|provider| provider.meta().name)
            .chain(LoginProviderName::ALL.map(LoginProviderName::as_str))
            .collect::<HashSet<_>>();
        if let Some(name) = selected_names.iter().find(|name| !known.contains(**name)) {
            return Err(usage(format!("unknown provider: {name}")));
        }
        if explicit {
            for name in LoginProviderName::ALL.map(LoginProviderName::as_str) {
                if selected_names.contains(name) && !stateful_ready(&store, name)? {
                    eprintln!("provider {name} requires login");
                    return Ok(EXIT_AUTH_REQUIRED);
                }
            }
        }
        available
            .into_iter()
            .filter(|provider| selected_names.contains(provider.meta().name))
            .collect()
    };
    let channels = if args.source == SourceSelection::Provider {
        Vec::new()
    } else {
        crate::channel::validate_search_channels(&config.search.channels)
            .map_err(|error| usage(error.to_string()))?
    };
    if providers.is_empty() && channels.is_empty() {
        return Err(usage("no search sources selected"));
    }
    if !args.quiet {
        eprintln!(
            "{}",
            search_defaults_text(
                providers.len(),
                channels.len(),
                config.search.jobs,
                timeout.as_secs(),
                config.search.all_timeout_secs
            )
        );
    }
    let context = SearchContext::new(http.clone(), timeout);
    let engine = SearchEngine::new(context);
    let options = SearchOptions {
        query: args.query,
        providers,
        channels,
        include: args.include,
        exclude: args.exclude,
        clouds: args.clouds,
        jobs: config.search.jobs,
        timeout,
        all_timeout: Duration::from_secs(config.search.all_timeout_secs),
    };
    let checker = if !args.no_check {
        let cache = if config.check.enabled_cache {
            paths.ensure_dirs()?;
            Some(CheckCache::open(&paths.check_cache)?)
        } else {
            None
        };
        Some(Arc::new(CheckEngine::new(http, cache)))
    } else {
        None
    };
    streaming::run(
        engine,
        options,
        checker,
        CheckOptions {
            jobs: config.check.jobs,
            timeout,
            refresh: false,
            no_cache: !config.check.enabled_cache,
            proxy_scope: proxy.as_deref().map(proxy_scope),
        },
        args.format,
        !args.no_check,
        args.quiet,
        crate::channel::ChannelStore::new(paths.channels_file.clone()),
    )
    .await
}

fn init_logging(verbose: bool, quiet: bool) {
    let level = if quiet {
        "off"
    } else if verbose {
        "debug"
    } else {
        "warn"
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(level)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

async fn run_check(args: CheckArgs, paths: &AppPaths, config: Config) -> anyhow::Result<i32> {
    if args.jobs == Some(0) {
        return Err(usage("--jobs must be greater than zero"));
    }
    let mut urls = args.urls;
    if args.stdin {
        urls.extend(io::stdin().lock().lines().collect::<Result<Vec<_>, _>>()?);
    }
    urls.retain(|url| !url.trim().is_empty());
    if urls.is_empty() {
        return Err(usage("at least one URL or --stdin is required"));
    }
    if args.password.is_some() && urls.len() != 1 {
        return Err(usage("--password is only valid with one URL"));
    }
    let (http, proxy, timeout, _) = client(&config, args.proxy, args.timeout)?;
    let explicit_type = args
        .cloud_type
        .as_deref()
        .map(crate::check::CheckCloudType::from_str)
        .transpose()
        .map_err(usage)?;
    let items = urls
        .into_iter()
        .map(|url| {
            let mut item = CheckItem::detect(url);
            if let Some(cloud) = explicit_type {
                item.cloud_type = cloud;
            }
            item.password = args.password.clone();
            item
        })
        .collect();
    let cache = if args.no_cache || !config.check.enabled_cache {
        None
    } else {
        paths.ensure_dirs()?;
        Some(CheckCache::open(&paths.check_cache)?)
    };
    let results = CheckEngine::new(http, cache)
        .check(
            items,
            CheckOptions {
                jobs: args.jobs.unwrap_or(config.check.jobs),
                timeout,
                refresh: args.refresh,
                no_cache: args.no_cache,
                proxy_scope: proxy.as_deref().map(proxy_scope),
            },
        )
        .await;
    let invalid = results
        .iter()
        .any(|result| matches!(result.state, CheckState::Bad | CheckState::Locked));
    write_checks(stdout(), &results, args.format)?;
    Ok(if args.fail_invalid && invalid {
        EXIT_INVALID_LINK
    } else {
        EXIT_OK
    })
}

async fn run_provider(
    args: ProviderArgs,
    paths: &AppPaths,
    config: &mut Config,
) -> anyhow::Result<i32> {
    let store = StateStore::from_paths(paths)?;
    match args.command {
        ProviderCommand::List => {
            println!("NAME         AUTH      READY");
            for provider in builtin_stateless_providers() {
                println!("{:<12} {:<9} yes", provider.meta().name, "no");
            }
            for provider in LoginProviderName::ALL {
                let name = provider.as_str();
                let auth = provider.auth_label();
                let count = store.list_profiles(name)?.len();
                let is_ready = stateful_ready(&store, name)?;
                let ready = if !is_ready {
                    "no".into()
                } else {
                    format!("yes ({count} profiles)")
                };
                println!("{name:<12} {auth:<9} {ready}");
            }
        }
        ProviderCommand::Profiles { name } => {
            require_stateful_name(&name)?;
            for profile in store.list_profiles(&name)? {
                println!("{profile}");
            }
        }
        ProviderCommand::Logout { name, profile } => {
            require_stateful_name(&name)?;
            store.delete(&name, &profile)?;
        }
        ProviderCommand::Status { name, profile } => {
            require_stateful_name(&name)?;
            let profiles = match profile {
                Some(value) => vec![value],
                None => store.list_profiles(&name)?,
            };
            for profile in profiles {
                let status = profile_status(&store, &name, &profile)?;
                println!("{name}/{profile}: {status}");
            }
        }
        ProviderCommand::Login(args) => {
            let name = args.name.as_str();
            let _ = store.profile_path(name, &args.profile)?;
            if args.remember_credentials
                && args.name.supports_remembered_credentials()
                && !store.encryption_enabled()
            {
                return Err(usage("--remember-credentials requires PANSOU_STATE_KEY"));
            }
            let (_, _, _, mut options) = client(config, None, None)?;
            match args.name {
                LoginProviderName::Qqpd => {
                    let factory = HttpClientFactory::new();
                    let session = factory.session(&options)?;
                    let provider = QqpdProvider::new(store.clone());
                    let auth = provider.auth_session(session);
                    let challenge = auth.begin().await?;
                    let _qr_file = if let Some(image) = challenge.terminal_escape_from_env() {
                        println!("{image}");
                        None
                    } else {
                        Some(show_qr_png("qqpd", &challenge.image_png)?)
                    };
                    loop {
                        tokio::select! {
                            signal = tokio::signal::ctrl_c() => {
                                signal?;
                                eprintln!("login cancelled");
                                return Ok(EXIT_OK);
                            }
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                        }
                        let poll = auth.poll(&challenge.qrsig).await?;
                        match poll.status {
                            QqQrStatus::Success => {
                                let cookie = poll.cookie.ok_or_else(|| {
                                    anyhow::anyhow!("QQPD login succeeded without cookies")
                                })?;
                                let qq_masked = poll.qq_masked.unwrap_or_default();
                                provider.save_login(&args.profile, cookie, qq_masked.clone())?;
                                if qq_masked.is_empty() {
                                    println!("qqpd/{}: logged in", args.profile);
                                } else {
                                    println!("qqpd/{}: logged in as {qq_masked}", args.profile);
                                }
                                break;
                            }
                            QqQrStatus::Expired => return Err(usage("QQPD QR code expired")),
                            QqQrStatus::Scanned => {
                                eprintln!("QR scanned; confirm login on the device")
                            }
                            QqQrStatus::Waiting => {}
                        }
                    }
                }
                LoginProviderName::Weibo => {
                    options.redirect = RedirectPolicy::None;
                    let session = HttpClientFactory::new().session(&options)?;
                    let provider = WeiboProvider::new(store.clone());
                    let auth = provider.auth_session(session);
                    let challenge = auth.begin().await?;
                    let _qr_file = if let Some(image) = challenge.terminal_escape_from_env() {
                        println!("{image}");
                        None
                    } else {
                        Some(show_qr_png("weibo", &challenge.image_png)?)
                    };
                    loop {
                        tokio::select! {
                            signal = tokio::signal::ctrl_c() => {
                                signal?;
                                eprintln!("login cancelled");
                                return Ok(EXIT_OK);
                            }
                            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                        }
                        let poll = auth.poll(&challenge.qrid).await?;
                        match poll.status {
                            QrStatus::Success => {
                                let cookie = poll.cookie.ok_or_else(|| {
                                    anyhow::anyhow!("Weibo login succeeded without cookies")
                                })?;
                                provider.save_login(&args.profile, cookie)?;
                                println!("weibo/{}: logged in", args.profile);
                                break;
                            }
                            QrStatus::Expired => return Err(usage("Weibo QR code expired")),
                            QrStatus::Scanned => {
                                eprintln!("QR scanned; confirm login on the device")
                            }
                            QrStatus::Waiting => {}
                        }
                    }
                }
                LoginProviderName::Gying => {
                    let username = required_username(args.username.as_deref())?;
                    let password = read_password(args.password_stdin)?;
                    let provider = GyingProvider::load(
                        store.clone(),
                        GyingEndpoints::parse(&config.providers.gying.base_url)?,
                        HttpClientFactory::new(),
                        options,
                    )?;
                    provider
                        .login(
                            &args.profile,
                            username,
                            &password,
                            args.remember_credentials,
                        )
                        .await?;
                    println!("gying/{}: logged in", args.profile);
                }
                LoginProviderName::Panlian => {
                    let username = required_username(args.username.as_deref())?;
                    let password = read_password(args.password_stdin)?;
                    let factory = HttpClientFactory::new();
                    let session = factory.session(&options)?;
                    let blocked = config
                        .providers
                        .panlian
                        .blocked_clouds
                        .iter()
                        .map(|v| CloudType::from_str(v))
                        .collect::<Result<Vec<_>, _>>()?;
                    let provider =
                        PanlianProvider::load(store.clone(), PanlianEndpoints::default(), blocked)?;
                    provider
                        .login(
                            &session,
                            &args.profile,
                            username,
                            &password,
                            args.remember_credentials,
                        )
                        .await?;
                    println!("panlian/{}: logged in", args.profile);
                }
            }
        }
        ProviderCommand::Configure(args) => {
            require_stateful_name(&args.name)?;
            let _ = store.profile_path(&args.name, &args.profile)?;
            match args.name.as_str() {
                "qqpd" => {
                    let (http, _, _, _) = client(config, None, None)?;
                    let provider = QqpdProvider::new(store.clone());
                    if !provider
                        .load_profile(&args.profile)?
                        .is_some_and(|profile| profile.is_ready(chrono::Utc::now()))
                    {
                        eprintln!("provider qqpd requires login");
                        return Ok(EXIT_AUTH_REQUIRED);
                    }
                    let channels = args.channels.into_iter().chain(args.channel);
                    let saved = provider
                        .configure_channels(&http, &args.profile, channels)
                        .await?;
                    println!("qqpd/{}: {} channels", args.profile, saved.len());
                }
                "weibo" => {
                    let provider = WeiboProvider::new(store.clone());
                    if !provider
                        .load_profile(&args.profile)?
                        .is_some_and(|profile| profile.is_ready(chrono::Utc::now()))
                    {
                        eprintln!("provider weibo requires login");
                        return Ok(EXIT_AUTH_REQUIRED);
                    }
                    let users = args.users.into_iter().chain(args.user);
                    let saved = provider.configure_users(&args.profile, users)?;
                    println!("weibo/{}: {} target users", args.profile, saved.len());
                }
                "gying" => {
                    let base = args
                        .base_url
                        .ok_or_else(|| usage("--base-url is required for gying"))?;
                    GyingEndpoints::parse(&base)?;
                    config.providers.gying.base_url = base;
                    save_config(paths, config)?;
                }
                "panlian" => {
                    for cloud in &args.blocked_clouds {
                        CloudType::from_str(cloud)?;
                    }
                    config.providers.panlian.blocked_clouds = args.blocked_clouds;
                    save_config(paths, config)?;
                }
                _ => return Err(usage(format!("unknown provider: {}", args.name))),
            }
        }
    }
    Ok(EXIT_OK)
}

fn require_stateful_name(name: &str) -> anyhow::Result<()> {
    if LoginProviderName::ALL
        .into_iter()
        .any(|provider| provider.as_str() == name)
    {
        Ok(())
    } else {
        Err(usage(format!("unknown stateful provider: {name}")))
    }
}

fn required_username(value: Option<&str>) -> anyhow::Result<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| usage("--username is required"))
}

fn read_password(from_stdin: bool) -> anyhow::Result<String> {
    let password = if from_stdin {
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        value.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        rpassword::prompt_password("Password: ")?
    };
    if password.is_empty() {
        return Err(usage("password must not be empty"));
    }
    Ok(password)
}

fn profile_status(
    store: &StateStore,
    provider: &str,
    profile: &str,
) -> anyhow::Result<&'static str> {
    let now = chrono::Utc::now();
    let status = match provider {
        "weibo" => match store.load::<WeiboProfile>(provider, profile)? {
            Some(value) if !value.is_ready(now) => "login required",
            Some(value) if value.user_ids.is_empty() => "configuration required",
            Some(_) => "ready",
            None => "login required",
        },
        "gying" => store
            .load::<GyingProfile>(provider, profile)?
            .filter(|value| value.status == "active" && !value.cookie.trim().is_empty())
            .map_or("login required", |_| "ready"),
        "panlian" => store
            .load::<PanlianProfile>(provider, profile)?
            .filter(|value| value.status == "active" && !value.cookie.trim().is_empty())
            .map_or("login required", |_| "ready"),
        "qqpd" => match store.load::<QqpdProfile>(provider, profile)? {
            Some(value) if !value.is_ready(now) => "login required",
            Some(value) if value.channels.is_empty() => "configuration required",
            Some(_) => "ready",
            None => "login required",
        },
        _ => "login required",
    };
    Ok(status)
}

fn save_config(paths: &AppPaths, config: &Config) -> anyhow::Result<()> {
    paths.ensure_dirs()?;
    let mut saved = toml::Value::try_from(config)?;
    // Provider configuration must not silently delete the deprecated field.
    if paths.config_file.exists() {
        let original: toml::Value = toml::from_str(&std::fs::read_to_string(&paths.config_file)?)?;
        if let Some(channels) = original.get("search").and_then(|s| s.get("channels")) {
            saved
                .get_mut("search")
                .and_then(toml::Value::as_table_mut)
                .expect("serialized config contains search table")
                .insert("channels".into(), channels.clone());
        }
    }
    let contents = toml::to_string_pretty(&saved)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&paths.config_dir)?;
    temporary.write_all(contents.as_bytes())?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(&paths.config_file)?;
    println!("saved {}", paths.config_file.display());
    Ok(())
}

fn show_qr_png(provider: &str, image: &[u8]) -> anyhow::Result<tempfile::TempPath> {
    let path = write_qr_png(provider, image)?;
    println!("Open the QR code and scan it: {}", path.display());
    open_with_system_viewer(&path);
    Ok(path)
}

fn write_qr_png(provider: &str, image: &[u8]) -> anyhow::Result<tempfile::TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix(&format!("pansou-{provider}-"))
        .suffix(".png")
        .tempfile()?;
    file.write_all(image)?;
    file.as_file_mut().sync_all()?;
    Ok(file.into_temp_path())
}

fn open_with_system_viewer(path: &Path) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = ProcessCommand::new("open");
        command.arg(path);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return;
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let _ = command.spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_fallback_uses_unique_temporary_png() {
        let first = write_qr_png("qqpd", b"png-one").unwrap();
        let second = write_qr_png("qqpd", b"png-two").unwrap();
        assert_ne!(first.as_os_str(), second.as_os_str());
        assert_eq!(std::fs::read(&first).unwrap(), b"png-one");
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("png")
        );
    }

    #[test]
    fn config_save_replaces_existing_file_without_fixed_temp_name() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_dirs(
            root.path().join("config"),
            root.path().join("state"),
            root.path().join("cache"),
        );
        let mut config = Config::default();
        save_config(&paths, &config).unwrap();
        config.network.timeout_secs = 17;
        save_config(&paths, &config).unwrap();
        let saved: Config =
            toml::from_str(&std::fs::read_to_string(&paths.config_file).unwrap()).unwrap();
        assert_eq!(saved.network.timeout_secs, 17);
    }
}
