//! Versioned, local-only migrations and actionable upgrade notices.
use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use fs2::FileExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: &'static str,
    pub message: &'static str,
}

/// Read-only compatibility check, also executed by a staged update binary.
pub fn preflight(config_dir: &Path) -> Result<()> {
    let path = config_dir.join("channels.toml");
    if path.exists() {
        let value: toml::Value = toml::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("cannot read channel format: {}", path.display()))?;
        let version = value.get("version").and_then(toml::Value::as_integer);
        if version != Some(1) {
            bail!(
                "unsupported channels.toml format {version:?}; this program supports version 1. Keep the newer program or restore a compatible backup"
            );
        }
    }
    Ok(())
}

/// Startup checks are idempotent. There is no channel conversion in this release.
pub fn check(config_dir: &Path) -> Result<Vec<Notice>> {
    preflight(config_dir)?;
    notices(config_dir)
}

/// Notices can be read independently when an unrelated data format is incompatible.
pub fn notices(config_dir: &Path) -> Result<Vec<Notice>> {
    let path = config_dir.join("config.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let value: toml::Value =
        toml::from_str(&fs::read_to_string(path)?).map_err(|error: toml::de::Error| {
            anyhow::anyhow!(
                "cannot inspect upgrade notices: config.toml {}",
                error.message()
            )
        })?;
    let old_channels = value
        .get("search")
        .and_then(|v| v.get("channels"))
        .is_some();
    Ok(if old_channels {
        vec![Notice {
            id: "independent-channels",
            message: "config.toml search.channels is ignored. Re-add channels with `pansou channel add <NAME>` or `pansou channel import <FILE_OR_URL>`, then remove the old field. No channels were automatically migrated.",
        }]
    } else {
        Vec::new()
    })
}

/// One deterministic schema step. Steps are keyed by data version, not app version.
pub struct Step {
    pub from: u32,
    pub to: u32,
    pub transform: fn(toml::Value) -> Result<toml::Value>,
}

/// Apply a complete chain in memory, back up the original, then atomically save.
/// Failed transforms leave the original untouched. No steps are registered for
/// channels v1: old config.toml channel lists deliberately require manual action.
pub fn migrate_file(path: &Path, target: u32, steps: &[Step]) -> Result<bool> {
    let lock_path = path.with_extension("lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    let original = fs::read(path)?;
    let mut value: toml::Value = toml::from_str(std::str::from_utf8(&original)?)?;
    let mut version = value
        .get("version")
        .and_then(toml::Value::as_integer)
        .and_then(|v| u32::try_from(v).ok())
        .context("missing or invalid data format version")?;
    if version > target {
        bail!("data format {version} is newer than supported format {target}");
    }
    if version == target {
        return Ok(false);
    }
    let initial = version;
    while version < target {
        let step = steps
            .iter()
            .find(|s| s.from == version && s.to > version && s.to <= target)
            .with_context(|| format!("no migration from format {version} to {target}"))?;
        value = (step.transform)(value)?;
        version = step.to;
        value
            .as_table_mut()
            .context("migration did not return a TOML table")?
            .insert("version".into(), toml::Value::Integer(i64::from(version)));
    }
    let parent = path.parent().context("data file has no parent")?;
    let mut backup = tempfile::Builder::new()
        .prefix(&format!(
            "{}.v{initial}.backup-",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
        .tempfile_in(parent)?;
    use std::io::Write;
    backup.write_all(&original)?;
    backup.as_file().sync_all()?;
    backup
        .keep()
        .context("could not preserve migration backup")?;
    let mut replacement = tempfile::NamedTempFile::new_in(parent)?;
    replacement
        .as_file()
        .set_permissions(fs::metadata(path)?.permissions())?;
    replacement.write_all(toml::to_string_pretty(&value)?.as_bytes())?;
    replacement.as_file().sync_all()?;
    replacement
        .persist(path)
        .context("could not save migrated data; original backup retained")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notices_follow_actual_old_field_without_copying() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[search]\nchannels = ['foo']\n",
        )
        .unwrap();
        assert_eq!(check(dir.path()).unwrap().len(), 1);
        assert!(!dir.path().join("channels.toml").exists());
        fs::write(dir.path().join("config.toml"), "[search]\njobs = 8\n").unwrap();
        assert!(check(dir.path()).unwrap().is_empty());
    }
    #[test]
    fn migrates_across_versions_once_and_preserves_failed_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.toml");
        fs::write(&path, "version = 1\nname = 'foo'\n").unwrap();
        let steps = [
            Step {
                from: 1,
                to: 2,
                transform: Ok,
            },
            Step {
                from: 2,
                to: 3,
                transform: Ok,
            },
        ];
        assert!(migrate_file(&path, 3, &steps).unwrap());
        assert!(!migrate_file(&path, 3, &steps).unwrap());
        let before = fs::read(&path).unwrap();
        let fail = [Step {
            from: 3,
            to: 4,
            transform: |_| bail!("invalid"),
        }];
        assert!(migrate_file(&path, 4, &fail).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(migrate_file(&path, 2, &[]).is_err());
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().contains("backup-"))
                .count(),
            1
        );
    }
}
