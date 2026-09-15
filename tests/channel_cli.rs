#![cfg(unix)]

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut fixture = Self {
            root,
            config: PathBuf::new(),
        };
        let output = fixture.command(&["config", "path"]).output().unwrap();
        assert!(output.status.success());
        fixture.config = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        assert!(
            fixture.config.starts_with(fixture.root.path()),
            "test configuration must be isolated"
        );
        fs::create_dir_all(fixture.config.parent().unwrap()).unwrap();
        fixture
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pansou"));
        command
            .args(args)
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"));
        for name in [
            "CHANNELS",
            "PANSOU_CHANNELS",
            "PANSOU_STATE_KEY",
            "PANSOU_PROVIDERS",
            "ENABLED_PLUGINS",
            "PANSOU_JOBS",
            "PANSOU_TIMEOUT_SECS",
            "PANSOU_ALL_TIMEOUT_SECS",
            "PANSOU_CHECK_JOBS",
            "PANSOU_PROXY",
            "PROXY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn channels(&self) -> PathBuf {
        self.config.with_file_name("channels.toml")
    }
}

#[test]
fn help_suggests_missing_builtin_candidates_without_changing_saved_choices() {
    let fixture = Fixture::new();
    let help = fixture.run(&["--help"]);
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(help.status.success());
    assert!(text.contains("pansou channel import --builtin"));
    assert!(text.contains("New entries are enabled by default"));
    assert!(!fixture.channels().exists());
    assert!(
        fixture
            .run(&["channel", "import", "--builtin"])
            .status
            .success()
    );
    let saved = fs::read(fixture.channels()).unwrap();
    let help = fixture.run(&["search", "--help"]);
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(!text.contains("Expand Telegram sources"));
    assert_eq!(fs::read(fixture.channels()).unwrap(), saved);
}

#[test]
fn import_defaults_to_enabled_and_bulk_actions_preserve_entries() {
    let fixture = Fixture::new();
    let input = fixture.root.path().join("channels.txt");
    fs::write(&input, "foo\nbar\n").unwrap();
    let source = input.to_str().unwrap();
    assert!(fixture.run(&["channel", "import", source]).status.success());
    let store = pansou::channel::ChannelStore::new(fixture.channels());
    assert_eq!(store.load().unwrap().enabled_names().len(), 3);
    assert!(
        fixture
            .run(&["channel", "disable", "--all"])
            .status
            .success()
    );
    assert!(store.load().unwrap().enabled_names().is_empty());
    assert!(fixture.run(&["channel", "import", source]).status.success());
    assert!(
        store.load().unwrap().enabled_names().is_empty(),
        "reimport must preserve existing states"
    );
    fs::write(&input, "baz\n").unwrap();
    assert!(
        fixture
            .run(&["channel", "import", source, "--disable"])
            .status
            .success()
    );
    assert!(store.load().unwrap().enabled_names().is_empty());
    assert!(
        fixture
            .run(&["channel", "enable", "--all"])
            .status
            .success()
    );
    assert_eq!(store.load().unwrap().enabled_names().len(), 4);
}

#[test]
fn builtin_import_can_start_enabled_or_disabled() {
    for disable in [false, true] {
        let fixture = Fixture::new();
        let mut args = vec!["channel", "import", "--builtin"];
        if disable {
            args.push("--disable");
        }
        assert!(fixture.run(&args).status.success());
        let list = pansou::channel::ChannelStore::new(fixture.channels())
            .load()
            .unwrap();
        assert_eq!(
            list.enabled_names().len(),
            if disable { 1 } else { list.channels.len() }
        );
    }
}

#[test]
fn independent_channels_ignore_old_config_and_keep_temporary_overrides_temporary() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.config,
        "[search]\nchannels = ['oldchannel']\nproviders = []\n",
    )
    .unwrap();
    fs::write(fixture.channels(), "version = 1\nchannels = []\n").unwrap();
    let output = fixture.run(&["channel", "add", "@Foo", "https://t.me/foo", "bar"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = fs::read(fixture.channels()).unwrap();
    let output = fixture
        .command(&["help"])
        .env("CHANNELS", "one,two,three")
        .env("PANSOU_CHANNELS", "onlyone")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Enabled channels: 1/128"));
    assert_eq!(fs::read(fixture.channels()).unwrap(), saved);
    let output = fixture.run(&["config", "show"]);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("oldchannel"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("search.channels is ignored"));
    assert!(
        fs::read_to_string(&fixture.config)
            .unwrap()
            .contains("oldchannel")
    );
    assert!(
        fixture
            .run(&["channel", "disable", "foo", "bar"])
            .status
            .success()
    );
    let output = fixture.run(&["search", "query", "--source", "tg"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no search sources"));
}

#[test]
fn help_reports_configured_budget_without_writing_and_survives_bad_config() {
    let fixture = Fixture::new();
    let list = pansou::channel::ChannelList {
        version: 1,
        channels: (0..128)
            .map(|i| pansou::channel::Channel {
                name: format!("channel{i}"),
                enabled: true,
            })
            .collect(),
    };
    let content = toml::to_string(&list).unwrap();
    fs::write(fixture.channels(), &content).unwrap();
    let output = fixture.run(&["help"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Providers: 15"));
    assert!(text.contains("~540s"));
    assert!(text.contains("longer queries may reduce search quality"));
    assert_eq!(fs::read_to_string(fixture.channels()).unwrap(), content);
    assert!(!fixture.channels().with_extension("lock").exists());
    fs::write(&fixture.config, "invalid = [").unwrap();
    let output = fixture.run(&["help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("unavailable"));
    assert!(
        fixture
            .run(&["channel", "disable", "channel0"])
            .status
            .success()
    );
    assert!(fixture.run(&["update", "--help"]).status.success());
}

#[test]
fn channel_limit_applies_to_cli_and_env_before_network_requests() {
    let fixture = Fixture::new();
    let names: Vec<_> = (0..129).map(|i| format!("channel{i}")).collect();
    let output = fixture
        .command(&["search", "query", "--source", "tg"])
        .env("PANSOU_CHANNELS", names.join(","))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("128"));
    let mut command = fixture.command(&["search", "query", "--source", "tg"]);
    for name in &names {
        command.arg("--channel").arg(name);
    }
    assert_eq!(command.output().unwrap().status.code(), Some(2));
}

#[test]
fn local_import_repairs_without_valid_main_config_and_is_transactional() {
    let fixture = Fixture::new();
    fs::write(&fixture.config, "[search]\njobs = 0\n").unwrap();
    let input = fixture.root.path().join("import.txt");
    fs::write(&input, "# test\nfoo\n@bar\nhttps://t.me/foo\n").unwrap();
    assert!(
        fixture
            .run(&["channel", "import", input.to_str().unwrap()])
            .status
            .success()
    );
    let before = fs::read(fixture.channels()).unwrap();
    fs::write(&input, "newchannel\nhttps://example.com/evil\n").unwrap();
    let output = fixture.run(&["channel", "import", input.to_str().unwrap()]);
    assert!(!output.status.success());
    assert_eq!(fs::read(fixture.channels()).unwrap(), before);
}

#[test]
fn incompatible_channels_do_not_block_unrelated_commands_or_explicit_overrides() {
    let fixture = Fixture::new();
    fs::write(fixture.channels(), "version = 99\nchannels = []\n").unwrap();
    fs::write(&fixture.config, "[search]\nproviders = []\n").unwrap();
    assert!(fixture.run(&["config", "show"]).status.success());
    let output = fixture.run(&["search", "query", "--source", "provider"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no search sources selected"));
    let output = fixture
        .command(&["search", "query", "--source", "tg"])
        .env("PANSOU_CHANNELS", "")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("no search sources selected"));
    let output = fixture
        .command(&["help"])
        .env("PANSOU_CHANNELS", "foo")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("Enabled channels: 1/128"));
}
