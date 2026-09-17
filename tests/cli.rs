use clap::Parser;
use pansou::cli::{
    Cli, Command, ConfigureProviderCommand, LoginProviderCommand, ProviderCommand,
    QqpdConfigureCommand, SourceSelection, WeiboConfigureCommand,
};
use pansou::output::OutputFormat;
use std::process::Command as ProcessCommand;

#[test]
fn visible_commands_have_english_descriptions() {
    use clap::CommandFactory;
    fn verify(command: &clap::Command) {
        for child in command.get_subcommands().filter(|c| !c.is_hide_set()) {
            let about = child
                .get_about()
                .expect("every command needs a description")
                .to_string();
            assert!(about.is_ascii(), "non-English command description: {about}");
            verify(child);
        }
    }
    let mut command = Cli::command();
    verify(&command);
    let help = command.render_long_help().to_string();
    assert!(help.contains("Use short, focused keywords"));
    assert!(help.is_ascii());
}

#[test]
fn parses_search_contract_and_repeatable_filters() {
    let cli = Cli::try_parse_from([
        "pansou",
        "search",
        "仙逆",
        "--source",
        "provider",
        "--provider",
        "meitizy",
        "--provider",
        "cyg",
        "--cloud",
        "quark",
        "--include",
        "4K",
        "--exclude",
        "枪版",
        "--format",
        "json",
        "--no-progress",
        "--jobs",
        "4",
        "--timeout",
        "12",
    ])
    .unwrap();
    let Command::Search(args) = cli.command else {
        panic!("expected search command")
    };
    assert_eq!(args.source, SourceSelection::Provider);
    assert_eq!(args.providers, ["meitizy", "cyg"]);
    assert_eq!(args.clouds[0].to_string(), "quark");
    assert_eq!(args.format, OutputFormat::Json);
    assert!(args.no_progress);
    assert_eq!(args.jobs, Some(4));
}

#[test]
fn check_stdin_parses() {
    let check = Cli::try_parse_from(["pansou", "check", "--stdin", "--fail-invalid"]).unwrap();
    assert!(matches!(check.command, Command::Check(args) if args.stdin && args.fail_invalid));
}

#[test]
fn search_checks_by_default_and_no_check_disables_it() {
    let default = Cli::try_parse_from(["pansou", "search", "仙逆"]).unwrap();
    assert!(matches!(default.command, Command::Search(args) if !args.no_check));

    let unchecked = Cli::try_parse_from(["pansou", "search", "仙逆", "--no-check"]).unwrap();
    assert!(matches!(unchecked.command, Command::Search(args) if args.no_check));

    for removed in ["--check", "--valid-only"] {
        assert!(Cli::try_parse_from(["pansou", "search", "仙逆", removed]).is_err());
    }
}

#[test]
fn search_help_describes_no_check() {
    let error = Cli::try_parse_from(["pansou", "search", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("--no-check"));
    assert!(help.contains("show all unchecked results"));
}

#[test]
fn provider_password_is_never_a_command_line_argument() {
    assert!(
        Cli::try_parse_from([
            "pansou",
            "provider",
            "login",
            "gying",
            "--username",
            "demo",
            "--password",
            "secret"
        ])
        .is_err()
    );
    let cli = Cli::try_parse_from([
        "pansou",
        "provider",
        "login",
        "gying",
        "--username",
        "demo",
        "--password-stdin",
    ])
    .unwrap();
    assert!(matches!(cli.command, Command::Provider(args)
        if matches!(args.command, ProviderCommand::Login(ref login)
            if matches!(login.provider, LoginProviderCommand::Gying(ref login)
                if login.password_stdin))));
}

#[test]
fn provider_login_help_lists_names_and_login_methods() {
    let error = Cli::try_parse_from(["pansou", "provider", "login", "--help"]).unwrap_err();
    let help = error.to_string();
    for name in ["qqpd", "weibo", "gying", "panlian"] {
        assert!(help.contains(name), "missing provider {name}: {help}");
    }
    assert!(help.contains("QR code login"));
    assert!(help.contains("Username/password login"));

    let error =
        Cli::try_parse_from(["pansou", "provider", "login", "gying", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("--username"));
    assert!(help.contains("--password-stdin"));
}

#[test]
fn weibo_login_help_explains_scan_and_target_user_setup() {
    let error =
        Cli::try_parse_from(["pansou", "provider", "login", "weibo", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Weibo mobile app"));
    assert!(help.contains("confirm on your phone"));
    assert!(help.contains("provider configure weibo add"));
    assert!(help.contains("UID_OR_PROFILE_URL"));
    assert!(help.contains("cloud-drive, magnet, or ed2k link"));
}

#[test]
fn qqpd_login_help_explains_scan_and_channel_setup() {
    let error = Cli::try_parse_from(["pansou", "provider", "login", "qqpd", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("QQ mobile app"));
    assert!(help.contains("confirm on your phone"));
    assert!(help.contains("provider configure qqpd add"));
    assert!(help.contains("CHANNEL_ID_OR_PD_URL"));
    assert!(help.contains("https://pd.qq.com/g/<CHANNEL_ID>"));
}

#[test]
fn parses_qqpd_channel_actions() {
    for action in ["add", "del", "list"] {
        let mut args = vec![
            "pansou",
            "provider",
            "configure",
            "qqpd",
            action,
            "--profile",
            "main",
        ];
        if action != "list" {
            args.extend(["--channel", "https://pd.qq.com/g/example"]);
        }
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Provider(provider) = cli.command else {
            panic!("expected provider command");
        };
        let ProviderCommand::Configure(configure) = provider.command else {
            panic!("expected configure command");
        };
        let ConfigureProviderCommand::Qqpd(qqpd) = configure.provider else {
            panic!("expected QQPD configuration");
        };
        let profile = match qqpd.action {
            QqpdConfigureCommand::Add(args) if action == "add" => args.profile,
            QqpdConfigureCommand::Del(args) if action == "del" => args.profile,
            QqpdConfigureCommand::List(args) if action == "list" => args.profile,
            _ => panic!("unexpected QQPD action"),
        };
        assert_eq!(profile, "main");
    }
}

#[test]
fn parses_weibo_target_user_actions() {
    for action in ["add", "del", "list"] {
        let mut args = vec![
            "pansou",
            "provider",
            "configure",
            "weibo",
            action,
            "--profile",
            "main",
        ];
        if action != "list" {
            args.extend(["--user", "https://weibo.com/u/1234567890"]);
        }
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Provider(provider) = cli.command else {
            panic!("expected provider command");
        };
        let ProviderCommand::Configure(configure) = provider.command else {
            panic!("expected configure command");
        };
        let ConfigureProviderCommand::Weibo(weibo) = configure.provider else {
            panic!("expected Weibo configuration");
        };
        let profile = match weibo.action {
            WeiboConfigureCommand::Add(args) if action == "add" => args.profile,
            WeiboConfigureCommand::Del(args) if action == "del" => args.profile,
            WeiboConfigureCommand::List(args) if action == "list" => args.profile,
            _ => panic!("unexpected Weibo action"),
        };
        assert_eq!(profile, "main");
    }
}

#[test]
fn weibo_configure_help_explains_how_to_find_a_uid() {
    let error = Cli::try_parse_from(["pansou", "provider", "configure", "weibo", "add", "--help"])
        .unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Add target users without removing existing ones"));
    assert!(help.contains("https://weibo.com/u/1234567890"));
    assert!(help.contains("Either the numeric UID or the full profile URL"));
}

#[test]
fn dynamic_search_defaults_only_appear_at_the_root() {
    let binary = env!("CARGO_BIN_EXE_pansou");
    for root_args in [vec!["help"], vec!["--help"], vec!["-h"], Vec::new()] {
        let output = ProcessCommand::new(binary)
            .args(root_args)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("Current search defaults:"), "{text}");
    }

    for subcommand_args in [
        vec!["search", "--help"],
        vec!["provider", "--help"],
        vec!["provider", "login", "--help"],
    ] {
        let output = ProcessCommand::new(binary)
            .args(subcommand_args)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("Current search defaults:"), "{text}");
    }
}

#[test]
fn verbose_and_quiet_conflict() {
    assert!(Cli::try_parse_from(["pansou", "search", "仙逆", "--verbose", "--quiet"]).is_err());
}

#[test]
fn search_and_check_use_json_and_reject_jsonl() {
    assert!(Cli::try_parse_from(["pansou", "search", "仙逆", "--format", "json"]).is_ok());
    let error = Cli::try_parse_from(["pansou", "search", "仙逆", "--format", "jsonl"]).unwrap_err();
    assert!(error.to_string().contains("use --format json"));
    assert!(Cli::try_parse_from(["pansou", "check", "--stdin", "--format", "json"]).is_ok());
    let error =
        Cli::try_parse_from(["pansou", "check", "--stdin", "--format", "jsonl"]).unwrap_err();
    assert!(error.to_string().contains("invalid value 'jsonl'"));
    let cli = Cli::try_parse_from(["pansou", "search", "仙逆", "--all-timeout", "600"]).unwrap();
    assert!(matches!(cli.command, Command::Search(args) if args.all_timeout == Some(600)));
}

#[test]
fn channel_import_and_update_arguments() {
    for source in ["./channels.txt", "https://example.com/channels.txt"] {
        assert!(Cli::try_parse_from(["pansou", "channel", "import", source]).is_ok());
    }
    assert!(Cli::try_parse_from(["pansou", "channel", "import", "--builtin"]).is_ok());
    assert!(Cli::try_parse_from(["pansou", "channel", "import"]).is_err());
    assert!(Cli::try_parse_from(["pansou", "channel", "import", "--builtin", "list.txt"]).is_err());
    assert!(Cli::try_parse_from(["pansou", "channel", "add", "@foo", "https://t.me/bar"]).is_ok());
    assert!(Cli::try_parse_from(["pansou", "update", "--check", "--version", "v0.2.0"]).is_ok());
}

#[test]
fn channel_bulk_actions_require_names_or_all_and_import_accepts_disable() {
    for action in ["enable", "disable"] {
        assert!(Cli::try_parse_from(["pansou", "channel", action, "--all"]).is_ok());
        assert!(Cli::try_parse_from(["pansou", "channel", action]).is_err());
        assert!(Cli::try_parse_from(["pansou", "channel", action, "--all", "foo"]).is_err());
    }
    for source in [
        "--builtin",
        "./channels.txt",
        "https://example.com/channels.txt",
    ] {
        assert!(Cli::try_parse_from(["pansou", "channel", "import", source, "--disable"]).is_ok());
    }
}
