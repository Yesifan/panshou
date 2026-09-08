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
