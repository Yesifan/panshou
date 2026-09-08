use std::path::{Path, PathBuf};

use directories::BaseDirs;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub providers_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub check_cache: PathBuf,
}

#[derive(Debug, Error)]
#[error("could not determine the platform user directories")]
pub struct PathError;

impl AppPaths {
    pub fn discover() -> Result<Self, PathError> {
        let base = BaseDirs::new().ok_or(PathError)?;
        let config_dir = base.config_dir().join("pansou");
        let state_base = base.state_dir().unwrap_or_else(|| base.data_local_dir());
        let state_dir = state_base.join("pansou");
        let cache_dir = base.cache_dir().join("pansou");
        Ok(Self::from_dirs(config_dir, state_dir, cache_dir))
    }

    /// Explicit paths are useful for tests and portable installations.
    pub fn from_dirs(
        config_dir: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        let config_dir = config_dir.into();
        let state_dir = state_dir.into();
        let cache_dir = cache_dir.into();
        Self {
            config_file: config_dir.join("config.toml"),
            providers_dir: state_dir.join("providers"),
            check_cache: cache_dir.join("check.redb"),
            config_dir,
            state_dir,
            cache_dir,
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        create_private_dir(&self.config_dir)?;
        create_private_dir(&self.providers_dir)?;
        create_private_dir(&self.cache_dir)
    }
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_documented_layout() {
        let paths = AppPaths::from_dirs("/config/pansou", "/state/pansou", "/cache/pansou");
        assert_eq!(paths.config_file, Path::new("/config/pansou/config.toml"));
        assert_eq!(paths.providers_dir, Path::new("/state/pansou/providers"));
        assert_eq!(paths.check_cache, Path::new("/cache/pansou/check.redb"));
    }
}
