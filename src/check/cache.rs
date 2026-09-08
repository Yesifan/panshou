use super::engine::CheckResult;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const RESULTS: TableDefinition<&str, &[u8]> = TableDefinition::new("check_results");

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("check cache database error: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("check cache transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("check cache table error: {0}")]
    Table(#[from] redb::TableError),
    #[error("check cache commit error: {0}")]
    Commit(#[from] redb::CommitError),
    #[error("check cache storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("check cache serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("cannot create check cache directory: {0}")]
    Directory(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct CheckCache {
    db: Arc<Database>,
}

impl CheckCache {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CacheError> {
        if let Some(parent) = path
            .as_ref()
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;
        let write = db.begin_write()?;
        {
            write.open_table(RESULTS)?;
        }
        write.commit()?;
        let cache = Self { db: Arc::new(db) };
        cache.prune_expired()?;
        Ok(cache)
    }

    pub fn get(&self, key: &str) -> Result<Option<CheckResult>, CacheError> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RESULTS)?;
        let Some(value) = table.get(key)? else {
            return Ok(None);
        };
        let result: CheckResult = serde_json::from_slice(value.value())?;
        drop(value);
        if result.expires_at_ms <= now_ms() {
            drop(table);
            drop(read);
            self.remove(key)?;
            return Ok(None);
        }
        Ok(Some(result))
    }

    pub fn put(&self, key: &str, result: &CheckResult) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(result)?;
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RESULTS)?;
            table.insert(key, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<(), CacheError> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RESULTS)?;
            table.remove(key)?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn prune_expired(&self) -> Result<usize, CacheError> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RESULTS)?;
        let mut stale = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            let expired = match serde_json::from_slice::<CheckResult>(value.value()) {
                Ok(value) => value.expires_at_ms <= now_ms(),
                Err(_) => true,
            };
            if expired {
                stale.push(key.value().to_owned());
            }
        }
        drop(table);
        drop(read);
        if stale.is_empty() {
            return Ok(0);
        }
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RESULTS)?;
            for key in &stale {
                table.remove(key.as_str())?;
            }
        }
        write.commit()?;
        Ok(stale.len())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{CheckCloudType, CheckState};

    fn sample(expires_at_ms: i64) -> CheckResult {
        CheckResult {
            cloud_type: CheckCloudType::Quark,
            url: "https://pan.quark.cn/s/a".into(),
            normalized_url: Some("https://pan.quark.cn/s/a".into()),
            state: CheckState::Ok,
            cache_hit: false,
            checked_at_ms: now_ms(),
            expires_at_ms,
            summary: Some("ok".into()),
        }
    }

    #[test]
    fn persists_and_prunes_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CheckCache::open(dir.path().join("check.redb")).unwrap();
        cache.put("fresh", &sample(now_ms() + 60_000)).unwrap();
        cache.put("stale", &sample(now_ms() - 1)).unwrap();
        assert!(cache.get("fresh").unwrap().is_some());
        assert!(cache.get("stale").unwrap().is_none());
        assert_eq!(cache.prune_expired().unwrap(), 0);
    }
}
