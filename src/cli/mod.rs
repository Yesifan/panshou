use std::{
    collections::HashSet,
    io::{self, BufRead, Read, Write},
    path::Path,
    process::Command as ProcessCommand,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{
    check::{CheckCache, CheckEngine, CheckItem, CheckOptions, CheckState, proxy_scope},
    config::{AppPaths, Config, ConfigOverrides},
    core::CloudType,
    http::{ClientOptions, HttpClientFactory, ProxyUrl, RedirectPolicy},
    output::{OutputFormat, stdout, write_checks, write_search},
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
    about = "Search and validate cloud-drive links"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Search(SearchArgs),
    Check(CheckArgs),
    Provider(ProviderArgs),
    Config(ConfigArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SourceSelection {
    All,
    Tg,
    Provider,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    pub query: String,
    #[arg(long, value_enum, default_value_t = SourceSelection::All)]
    pub source: SourceSelection,
    #[arg(long = "provider")]
    pub providers: Vec<String>,
    #[arg(long = "channel")]
    pub channels: Vec<String>,
    #[arg(long = "cloud")]
    pub clouds: Vec<CloudType>,
    #[arg(long = "include")]
    pub include: Vec<String>,
    #[arg(long = "exclude")]
    pub exclude: Vec<String>,
    #[arg(long)]
    pub jobs: Option<usize>,
    #[arg(long)]
    pub timeout: Option<u64>,
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub valid_only: bool,
    #[arg(long, conflicts_with = "quiet")]
    pub verbose: bool,
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    pub urls: Vec<String>,
    #[arg(long)]
    pub stdin: bool,
    #[arg(long = "type")]
    pub cloud_type: Option<String>,
    #[arg(long)]
    pub password: Option<String>,
    #[arg(long)]
    pub jobs: Option<usize>,
    #[arg(long)]
    pub timeout: Option<u64>,
    #[arg(long)]
    pub proxy: Option<String>,
    #[arg(long)]
    pub refresh: bool,
    #[arg(long)]
    pub no_cache: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
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
    List,
    Profiles {
        name: String,
    },
    Login(LoginArgs),
    Logout {
        name: String,
        #[arg(long, default_value = "main")]
        profile: String,
    },
    Status {
        name: String,
        #[arg(long)]
        profile: Option<String>,
    },
    Configure(ConfigureArgs),
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    pub name: String,
    #[arg(long, default_value = "main")]
    pub profile: String,
    #[arg(long)]
    pub username: Option<String>,
    #[arg(long)]
    pub password_stdin: bool,
    #[arg(long)]
    pub remember_credentials: bool,
}

#[derive(Debug, Args)]
pub struct ConfigureArgs {
    pub name: String,
    #[arg(long, default_value = "main")]
    pub profile: String,
    #[arg(long, value_delimiter = ',')]
    pub channels: Vec<String>,
    #[arg(long = "channel")]
    pub channel: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub users: Vec<String>,
    #[arg(long = "user")]
    pub user: Vec<String>,
    #[arg(long)]
    pub base_url: Option<String>,
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
    Show,
    Path,
}

pub async fn run(cli: Cli) -> anyhow::Result<i32> {
    let paths = AppPaths::discover()?;
    let mut config = Config::load(&paths)?;
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
    }
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
        search_jobs: args.jobs,
        channels: (!args.channels.is_empty()).then(|| args.channels.clone()),
        providers: (!args.providers.is_empty()).then(|| args.providers.clone()),
        check_jobs: None,
    })?;
    let (http, proxy, timeout, client_options) = client(&config, args.proxy, args.timeout)?;
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
        for name in ["qqpd", "weibo", "gying", "panlian"] {
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
        .chain(["qqpd", "weibo", "gying", "panlian"])
        .collect::<HashSet<_>>();
    if let Some(name) = selected_names.iter().find(|name| !known.contains(**name)) {
        return Err(usage(format!("unknown provider: {name}")));
    }
    if explicit {
        for name in ["qqpd", "weibo", "gying", "panlian"] {
            if selected_names.contains(name) && !stateful_ready(&store, name)? {
                eprintln!("provider {name} requires login");
                return Ok(EXIT_AUTH_REQUIRED);
            }
        }
    }
    let providers = if args.source == SourceSelection::Tg {
        Vec::new()
    } else {
        available
            .into_iter()
            .filter(|provider| selected_names.contains(provider.meta().name))
            .collect()
    };
    let channels = if args.source == SourceSelection::Provider {
        Vec::new()
    } else {
        config.search.channels.clone()
    };
    if providers.is_empty() && channels.is_empty() {
        return Err(usage("no search sources selected"));
    }
    let context = SearchContext::new(http.clone(), timeout);
    let mut outcome = SearchEngine::new(context)
        .search(SearchOptions {
            query: args.query,
            providers,
            channels,
            include: args.include,
            exclude: args.exclude,
            clouds: args.clouds,
            jobs: config.search.jobs,
            timeout,
        })
        .await;

    if args.check || args.valid_only {
        let cache = if config.check.enabled_cache {
            paths.ensure_dirs()?;
            Some(CheckCache::open(&paths.check_cache)?)
        } else {
            None
        };
        let checker = CheckEngine::new(http, cache);
        let links = outcome
            .links_by_type
            .values()
            .flat_map(|links| links.iter())
            .map(|link| {
                let mut item = CheckItem::detect(link.url.clone());
                item.password = link.password.clone();
                item
            })
            .collect();
        let checked = checker
            .check(
                links,
                CheckOptions {
                    jobs: config.check.jobs,
                    timeout,
                    refresh: false,
                    no_cache: !config.check.enabled_cache,
                    proxy_scope: proxy.as_deref().map(proxy_scope),
                },
            )
            .await;
        let mut by_url = std::collections::HashMap::new();
        for result in checked {
            by_url.insert(result.url.clone(), result);
        }
        for links in outcome.links_by_type.values_mut() {
            for link in links.iter_mut() {
                if let Some(value) = by_url.get(&link.url) {
                    link.check = Some(crate::core::CheckResult {
                        state: match value.state {
                            CheckState::Ok => crate::core::CheckState::Ok,
                            CheckState::Bad => crate::core::CheckState::Bad,
                            CheckState::Locked => crate::core::CheckState::Locked,
                            CheckState::Unsupported => crate::core::CheckState::Unsupported,
                            CheckState::Uncertain => crate::core::CheckState::Uncertain,
                        },
                        cache_hit: value.cache_hit,
                        checked_at: None,
                        expires_at: None,
                        summary: value.summary.clone(),
                    });
                }
            }
            if args.valid_only {
                links.retain(|link| {
                    link.check
                        .as_ref()
                        .is_some_and(|check| check.state == crate::core::CheckState::Ok)
                });
            }
        }
        outcome.links_by_type.retain(|_, links| !links.is_empty());
        outcome.total_links = outcome.links_by_type.values().map(Vec::len).sum();
        if args.valid_only {
            let valid_urls = outcome
                .links_by_type
                .values()
                .flatten()
                .map(|link| crate::core::canonical_url_key(&link.url))
                .collect::<HashSet<_>>();
            outcome.results.retain_mut(|result| {
                result
                    .links
                    .retain(|link| valid_urls.contains(&crate::core::canonical_url_key(&link.url)));
                !result.links.is_empty()
            });
            outcome.total_results = outcome.results.len();
        }
    }
    for error in &outcome.source_errors {
        if !args.quiet {
            eprintln!("{}: {}", error.source, error.message);
        }
    }
    write_search(stdout(), &outcome, args.format)?;
    Ok(if outcome.successful_sources == 0 {
        EXIT_SEARCH_FAILED
    } else {
        EXIT_OK
    })
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
            for (name, auth) in [
                ("qqpd", "qr"),
                ("weibo", "qr"),
                ("gying", "password"),
                ("panlian", "password"),
            ] {
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
            require_stateful_name(&args.name)?;
            let _ = store.profile_path(&args.name, &args.profile)?;
            if args.remember_credentials
                && matches!(args.name.as_str(), "gying" | "panlian")
                && !store.encryption_enabled()
            {
                return Err(usage("--remember-credentials requires PANSOU_STATE_KEY"));
            }
            let (_, _, _, mut options) = client(config, None, None)?;
            match args.name.as_str() {
                "qqpd" => {
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
                "weibo" => {
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
                "gying" => {
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
                "panlian" => {
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
                _ => return Err(usage(format!("unknown provider: {}", args.name))),
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
    if ["qqpd", "weibo", "gying", "panlian"].contains(&name) {
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
    let contents = toml::to_string_pretty(config)?;
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
