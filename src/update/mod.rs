//! Release discovery and verified self replacement. Network endpoints are injectable
//! for tests; installation always targets the current executable in production.
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const API: &str = "https://api.github.com/repos/Yesifan/panshou";
const MAX_ARCHIVE: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone)]
pub struct UpdatePlan {
    pub current: Version,
    pub target: Version,
    pub notes: String,
    pub needs_install: bool,
    pub asset_name: String,
    archive_url: String,
    checksum_url: String,
}

#[derive(Debug)]
pub enum InstallOutcome {
    Installed {
        version: Version,
        backup: PathBuf,
        notices: Vec<String>,
    },
    /// Windows completes replacement in the staged executable after parent exits.
    Scheduled { version: Version },
    InstalledMigrationFailed {
        version: Version,
        backup: PathBuf,
        error: String,
    },
}

pub struct UpdateClient {
    client: Client,
    api: String,
}

impl UpdateClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            api: API.into(),
        }
    }

    pub fn with_api(client: Client, api: impl Into<String>) -> Self {
        Self {
            client,
            api: api.into().trim_end_matches('/').into(),
        }
    }

    pub async fn check(&self, current: &str, requested: Option<&str>) -> Result<UpdatePlan> {
        let current = parse_version(current)?;
        let endpoint = if let Some(version) = requested {
            format!("{}/releases/tags/v{}", self.api, parse_version(version)?)
        } else {
            format!("{}/releases/latest", self.api)
        };
        let bytes = self
            .download(&endpoint, 4 * 1024 * 1024)
            .await
            .context("release check failed")?;
        let release: Release =
            serde_json::from_slice(&bytes).context("invalid GitHub release response")?;
        let target = parse_version(&release.tag_name)?;
        if release.draft || (requested.is_none() && (release.prerelease || !target.pre.is_empty()))
        {
            bail!("release is not a published stable version");
        }
        if let Some(expected) = requested
            && target != parse_version(expected)?
        {
            bail!("release version does not match requested version");
        }
        let asset_name = platform_asset(std::env::consts::OS, std::env::consts::ARCH)?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .with_context(|| {
                format!("release {target} has no asset for this platform ({asset_name})")
            })?;
        let checksums = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS")
            .context("release has no SHA256SUMS; cannot safely install")?;
        Ok(UpdatePlan {
            needs_install: if requested.is_some() {
                target != current
            } else {
                target > current
            },
            current,
            target,
            notes: release.body.unwrap_or_default(),
            asset_name,
            archive_url: asset.browser_download_url.clone(),
            checksum_url: checksums.browser_download_url.clone(),
        })
    }

    async fn download(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let mut response = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| e.without_url())?
            .error_for_status()
            .map_err(|e| e.without_url())?;
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            bail!("download exceeds size limit");
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url())? {
            if data.len() + chunk.len() > limit {
                bail!("download exceeds size limit");
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    pub async fn install(&self, plan: &UpdatePlan, config_dir: &Path) -> Result<InstallOutcome> {
        let target = std::env::current_exe()?.canonicalize()?;
        self.install_at(plan, config_dir, &target).await
    }

    // Explicit target is private so integration tests cannot accidentally replace
    // the test runner; production always resolves current_exe above.
    async fn install_at(
        &self,
        plan: &UpdatePlan,
        config_dir: &Path,
        target: &Path,
    ) -> Result<InstallOutcome> {
        if !plan.needs_install {
            bail!(
                "target version is already installed or older than the current stable installation"
            );
        }
        validate_install_location(target)?;
        let parent = target
            .parent()
            .context("executable has no installation directory")?;
        let staging = tempfile::Builder::new().prefix(".pansou-update-").tempdir_in(parent)
            .with_context(|| format!("installation directory {} is not writable; update using the original installer or choose a user-writable installation", parent.display()))?;
        let sums = self
            .download(&plan.checksum_url, 1024 * 1024)
            .await
            .context("checksum download failed")?;
        let archive = self
            .download(&plan.archive_url, MAX_ARCHIVE)
            .await
            .context("program download failed")?;
        verify_checksum(&archive, std::str::from_utf8(&sums)?, &plan.asset_name)?;
        let staged = staging.path().join(if cfg!(windows) {
            "pansou.exe"
        } else {
            "pansou"
        });
        extract_binary(&archive, &plan.asset_name, &staged)?;
        verify_binary(&staged, &plan.target, config_dir)?;

        #[cfg(windows)]
        {
            let staged_dir = staging.keep();
            match Command::new(&staged)
                .arg("__update-replace")
                .arg(&staged)
                .arg(target)
                .arg(config_dir)
                .spawn()
            {
                Ok(_) => {
                    return Ok(InstallOutcome::Scheduled {
                        version: plan.target.clone(),
                    });
                }
                Err(error) => {
                    let _ = fs::remove_dir_all(staged_dir);
                    return Err(error).context("could not start update replacement helper");
                }
            }
        }
        #[cfg(not(windows))]
        {
            let backup = replace_binary(&staged, target)?;
            Ok(finish_install(
                target,
                config_dir,
                plan.target.clone(),
                backup,
            ))
        }
    }
}

fn parse_version(value: &str) -> Result<Version> {
    Version::parse(value.strip_prefix('v').unwrap_or(value)).context("invalid semantic version")
}

pub fn platform_asset(os: &str, arch: &str) -> Result<String> {
    let (platform, extension) = match (os, arch) {
        ("linux", "x86_64" | "aarch64") => ("linux", "tar.gz"),
        ("macos", "x86_64" | "aarch64") => ("macos", "tar.gz"),
        ("windows", "x86_64") => ("windows", "zip"),
        _ => bail!("no published update asset for {os}/{arch}"),
    };
    Ok(format!("pansou-{platform}-{arch}.{extension}"))
}

pub fn verify_checksum(bytes: &[u8], sums: &str, name: &str) -> Result<()> {
    let matches: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let checksum = columns.next()?;
            let file = columns.next()?.trim_start_matches('*');
            (file == name).then_some(checksum)
        })
        .collect();
    if matches.len() != 1 {
        bail!("expected exactly one SHA256SUMS entry for {name}");
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !matches[0].eq_ignore_ascii_case(&actual) {
        bail!("SHA-256 verification failed for {name}; current installation was not changed");
    }
    Ok(())
}

fn extract_binary(bytes: &[u8], asset: &str, destination: &Path) -> Result<()> {
    let mut contents = Vec::new();
    let expected = if asset.ends_with(".zip") {
        "pansou.exe"
    } else {
        "pansou"
    };
    let mut found = false;
    if asset.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            if entry.name() == expected {
                if found
                    || !entry.is_file()
                    || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
                {
                    bail!("invalid program archive entry");
                }
                found = true;
                (&mut entry)
                    .take(MAX_ARCHIVE as u64 + 1)
                    .read_to_end(&mut contents)?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path()?.as_ref() == Path::new(expected) {
                if found || !entry.header().entry_type().is_file() {
                    bail!("invalid program archive entry");
                }
                found = true;
                (&mut entry)
                    .take(MAX_ARCHIVE as u64 + 1)
                    .read_to_end(&mut contents)?;
            }
        }
    }
    if !found || contents.is_empty() || contents.len() > MAX_ARCHIVE {
        bail!("archive has no valid {expected} binary");
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(&contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all()?;
    Ok(())
}

fn verify_binary(binary: &Path, expected: &Version, config_dir: &Path) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .context("could not run downloaded binary")?;
    if !output.status.success() {
        bail!("downloaded binary version check failed");
    }
    let version = String::from_utf8(output.stdout)?;
    let reported = version
        .split_whitespace()
        .last()
        .context("missing binary version")?;
    if parse_version(reported)? != *expected {
        bail!("downloaded binary version differs from release version");
    }
    let preflight = Command::new(binary)
        .arg("__update-preflight")
        .arg(config_dir)
        .output()?;
    if !preflight.status.success() {
        bail!(
            "target version cannot confirm local data compatibility; current installation retained. {}",
            String::from_utf8_lossy(&preflight.stderr)
        );
    }
    Ok(())
}

fn validate_install_location(path: &Path) -> Result<()> {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    for (marker, manager) in [
        ("/cellar/", "Homebrew (brew upgrade)"),
        ("/scoop/", "Scoop (scoop update)"),
        ("/chocolatey/", "Chocolatey (choco upgrade)"),
        ("/winget/packages/", "WinGet (winget upgrade)"),
        ("/nix/store/", "Nix"),
        ("/snap/", "Snap"),
    ] {
        if normalized.contains(marker) {
            bail!(
                "{} is managed by {manager}; update with that package manager",
                path.display()
            );
        }
    }
    if normalized.contains("/target/debug/") || normalized.contains("/target/release/") {
        bail!(
            "{} is a development build; rebuild it with Cargo or install a release binary before self-updating",
            path.display()
        );
    }
    #[cfg(target_os = "linux")]
    for (program, flag, manager) in [
        ("dpkg-query", "--search", "the Debian package manager"),
        ("rpm", "-qf", "the RPM package manager"),
    ] {
        if Command::new(program)
            .arg(flag)
            .arg(path)
            .output()
            .is_ok_and(|output| output.status.success())
        {
            bail!(
                "{} is owned by {manager}; update it through the package manager",
                path.display()
            );
        }
    }
    Ok(())
}

fn replace_binary(staged: &Path, target: &Path) -> Result<PathBuf> {
    let mut backup = tempfile::Builder::new()
        .prefix(".pansou-backup-")
        .tempfile_in(target.parent().context("installation has no parent")?)?;
    let mut source = fs::File::open(target)?;
    std::io::copy(&mut source, &mut backup)?;
    backup
        .as_file()
        .set_permissions(source.metadata()?.permissions())?;
    backup.as_file().sync_all()?;
    drop(source);
    let (_, backup_path) = backup
        .keep()
        .context("could not preserve previous program")?;
    #[cfg(windows)]
    {
        fs::remove_file(target).context("could not remove old executable after it exited")?;
        if let Err(error) = fs::rename(staged, target) {
            fs::copy(&backup_path, target).context(
                "replacement and program restoration failed; restore the retained backup",
            )?;
            return Err(error).context("replacement failed; previous program restored");
        }
    }
    #[cfg(not(windows))]
    fs::rename(staged, target).context("replacement failed; previous program retained")?;
    Ok(backup_path)
}

fn finish_install(
    binary: &Path,
    config_dir: &Path,
    version: Version,
    backup: PathBuf,
) -> InstallOutcome {
    match Command::new(binary)
        .arg("__update-finish")
        .arg(config_dir)
        .output()
    {
        Ok(output) if output.status.success() => InstallOutcome::Installed {
            version,
            backup,
            notices: String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_owned)
                .collect(),
        },
        result => InstallOutcome::InstalledMigrationFailed {
            version,
            backup,
            error: match result {
                Ok(output) => String::from_utf8_lossy(&output.stderr).into_owned(),
                Err(error) => error.to_string(),
            },
        },
    }
}

/// Hidden Windows helper. Runs from a copy separate from the staged replacement,
/// retries sharing violations until the original process has exited.
pub fn replace_after_exit(
    staged: &Path,
    target: &Path,
    config_dir: &Path,
) -> Result<InstallOutcome> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let replacement = staged.with_file_name("pansou-replacement.exe");
        fs::copy(staged, &replacement)?;
        let version = Version::parse(env!("CARGO_PKG_VERSION"))?;
        for attempt in 0..100 {
            match fs::OpenOptions::new().read(true).write(true).share_mode(0).open(target) {
                Ok(handle) => {
                    drop(handle);
                    let backup = replace_binary(&replacement, target)?;
                    return Ok(finish_install(target, config_dir, version, backup));
                }
                Err(error) if attempt == 99 => return Err(error).context("original program did not release its executable; retry update after closing other pansou processes"),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        unreachable!()
    }
    #[cfg(not(windows))]
    {
        let _ = (staged, target, config_dir);
        bail!("replacement helper is only used on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksums_reject_corruption_and_duplicates() {
        let sum = format!("{:x}  example\n", Sha256::digest(b"binary"));
        verify_checksum(b"binary", &sum, "example").unwrap();
        assert!(verify_checksum(b"changed", &sum, "example").is_err());
        assert!(verify_checksum(b"binary", &(sum.clone() + &sum), "example").is_err());
    }
    #[test]
    fn platforms_and_managed_installations() {
        assert_eq!(
            platform_asset("windows", "x86_64").unwrap(),
            "pansou-windows-x86_64.zip"
        );
        assert!(platform_asset("windows", "aarch64").is_err());
        assert!(
            validate_install_location(Path::new("/opt/homebrew/Cellar/pansou/1/bin/pansou"))
                .is_err()
        );
        assert!(validate_install_location(Path::new("/home/user/.local/bin/pansou")).is_ok());
    }
    #[test]
    fn extraction_ignores_paths_and_replacement_preserves_backup() {
        let dir = tempfile::tempdir().unwrap();
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "pansou", &b"new"[..])
            .unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&archive.into_inner().unwrap()).unwrap();
        let staged = dir.path().join("staged");
        extract_binary(&gzip.finish().unwrap(), "asset.tar.gz", &staged).unwrap();
        let target = dir.path().join("installed");
        fs::write(&target, "old").unwrap();
        let backup = replace_binary(&staged, &target).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"new");
        assert_eq!(fs::read(backup).unwrap(), b"old");
    }
    #[tokio::test]
    async fn release_checks_are_mockable_and_do_not_install() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        let asset = platform_asset(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        Mock::given(method("GET")).and(path("/releases/latest")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tag_name":"v0.2.0", "body":"Upgrade notes", "assets":[{"name":asset,"browser_download_url":"https://example.invalid/archive"},{"name":"SHA256SUMS","browser_download_url":"https://example.invalid/sums"}]
        }))).expect(2).mount(&server).await;
        let updater = UpdateClient::with_api(Client::new(), server.uri());
        assert!(updater.check("0.1.0", None).await.unwrap().needs_install);
        assert!(!updater.check("0.3.0", None).await.unwrap().needs_install);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_install_checks_version_and_compatibility_before_replacing() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("installed-pansou");
        fs::write(&target, "original program").unwrap();
        let updater = UpdateClient::with_api(Client::new(), server.uri());
        let mut plan = UpdatePlan {
            current: Version::new(0, 1, 0),
            target: Version::new(0, 2, 0),
            notes: String::new(),
            needs_install: true,
            asset_name: "pansou-test.tar.gz".into(),
            archive_url: format!("{}/archive", server.uri()),
            checksum_url: format!("{}/sums", server.uri()),
        };
        let script = b"#!/bin/sh\ncase \"$1\" in\n--version) echo 'pansou 0.2.0';;\n__update-preflight) exit 0;;\n__update-finish) echo 'migration checked';;\n*) exit 2;;\nesac\n";
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "pansou", &script[..])
            .unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&archive.into_inner().unwrap()).unwrap();
        let bytes = gzip.finish().unwrap();
        let sums = format!("{:x}  {}", Sha256::digest(&bytes), plan.asset_name);
        Mock::given(path("/archive"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;
        Mock::given(path("/sums"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sums))
            .mount(&server)
            .await;

        plan.target = Version::new(0, 3, 0);
        assert!(
            updater
                .install_at(&plan, dir.path(), &target)
                .await
                .unwrap_err()
                .to_string()
                .contains("version differs")
        );
        assert_eq!(fs::read(&target).unwrap(), b"original program");
        plan.target = Version::new(0, 2, 0);
        let outcome = updater
            .install_at(&plan, dir.path(), &target)
            .await
            .unwrap();
        match outcome {
            InstallOutcome::Installed {
                version,
                backup,
                notices,
            } => {
                assert_eq!(version, plan.target);
                assert_eq!(fs::read(backup).unwrap(), b"original program");
                assert_eq!(notices, ["migration checked"]);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(fs::read(target).unwrap(), script);
    }

    #[test]
    fn zip_extraction_selects_only_program_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("../outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"ignored").unwrap();
        archive
            .start_file("pansou.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"windows binary").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let target = dir.path().join("staged.exe");
        extract_binary(&bytes, "windows.zip", &target).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"windows binary");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_target_is_rejected_before_installation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("candidate");
        fs::write(&binary, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'pansou 0.1.0'; else echo 'unsupported local schema' >&2; exit 2; fi\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let error = verify_binary(&binary, &Version::new(0, 1, 0), dir.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot confirm local data compatibility")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_waits_for_sharing_lock_before_replacing() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("installed.exe");
        let staged = dir.path().join("staged.exe");
        fs::write(&target, b"old program").unwrap();
        fs::copy(std::env::current_exe().unwrap(), &staged).unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            drop(held);
        });
        let result = replace_after_exit(&staged, &target, dir.path()).unwrap();
        release.join().unwrap();
        // The test runner deliberately does not implement the migration CLI.
        // Replacement must nevertheless finish and accurately report that stage.
        let InstallOutcome::InstalledMigrationFailed { backup, .. } = result else {
            panic!("test runner unexpectedly handled migration command")
        };
        assert_eq!(fs::read(backup).unwrap(), b"old program");
        assert_eq!(fs::read(&target).unwrap(), fs::read(staged).unwrap());
    }
}
