use clap::Parser;
use pansou::cli::{Cli, Command, ProviderCommand, SourceSelection};
use pansou::output::OutputFormat;

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
        "jsonl",
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
    assert_eq!(args.format, OutputFormat::Jsonl);
    assert_eq!(args.jobs, Some(4));
}

#[test]
fn valid_only_and_check_stdin_parse() {
    let search = Cli::try_parse_from(["pansou", "search", "仙逆", "--valid-only"]).unwrap();
    assert!(matches!(search.command, Command::Search(args) if args.valid_only));
    let check = Cli::try_parse_from(["pansou", "check", "--stdin", "--fail-invalid"]).unwrap();
    assert!(matches!(check.command, Command::Check(args) if args.stdin && args.fail_invalid));
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
        if matches!(args.command, ProviderCommand::Login(ref login) if login.password_stdin)));
}

#[test]
fn verbose_and_quiet_conflict() {
    assert!(Cli::try_parse_from(["pansou", "search", "仙逆", "--verbose", "--quiet"]).is_err());
}

#[test]
fn search_json_is_removed_but_check_json_remains() {
    let error = Cli::try_parse_from(["pansou", "search", "仙逆", "--format", "json"]).unwrap_err();
    assert!(error.to_string().contains("use --format jsonl"));
    assert!(Cli::try_parse_from(["pansou", "check", "--stdin", "--format", "json"]).is_ok());
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
