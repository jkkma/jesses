//! Versioned local job history. Recovery never restarts tools or touches media.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use media_core::{AppError, JobSnapshot};
use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_JOBS: usize = 100;
static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(super) struct History {
    path: PathBuf,
    _lock: Arc<HistoryLock>,
}

struct HistoryLock {
    _file: fs::File,
}

#[cfg(unix)]
impl Drop for HistoryLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // A concurrent fork can inherit this open file description until exec
        // applies CLOEXEC. Closing our last descriptor alone leaves flock held
        // by that child. Release it explicitly when the final history/write
        // owner drops, so a new manager can reopen the history immediately.
        // SAFETY: the uniquely owned file remains open throughout this call.
        unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    jobs: Vec<JobSnapshot>,
}

fn error(path: &Path, detail: impl std::fmt::Display) -> AppError {
    super::files::error(
        "JOB_HISTORY_FAILED",
        format!(
            "Job history at {} could not be read or saved: {detail}. Existing history and media were preserved.",
            path.display()
        ),
        path,
    )
}

impl History {
    pub async fn open(directory: PathBuf) -> Result<(Self, Vec<JobSnapshot>), AppError> {
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&directory).map_err(|e| error(&directory, e))?;
            let lock_path = directory.join("jobs.lock");
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0);
            }
            let lock = options.open(&lock_path).map_err(|e| {
                error(
                    &lock_path,
                    format!("another instance may be using this history: {e}"),
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                // The exclusive lock lives as long as the manager and its writes.
                if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    return Err(error(&lock_path, "another instance is using this history"));
                }
            }
            let history = Self {
                path: directory.join("jobs.json"),
                _lock: Arc::new(HistoryLock { _file: lock }),
            };
            let jobs = history.read()?;
            Ok((history, jobs))
        })
        .await
        .map_err(|e| AppError::new("JOB_HISTORY_FAILED", e.to_string(), None))?
    }

    fn read(&self) -> Result<Vec<JobSnapshot>, AppError> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(error(&self.path, e)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| error(&self.path, e))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(error(&self.path, "history exceeds its size limit"));
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|e| error(&self.path, e))?;
        if record.version != 1 || record.jobs.len() > MAX_JOBS {
            return Err(error(
                &self.path,
                "unsupported history version or record count",
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for job in &record.jobs {
            if job.id.is_empty() || !ids.insert(&job.id) {
                return Err(error(&self.path, "invalid or repeated job identifier"));
            }
        }
        Ok(record.jobs)
    }

    // The manager serializes calls while holding its state lock, so an older
    // snapshot cannot win a race against a terminal-state write.
    pub async fn save(&self, jobs: Vec<JobSnapshot>) -> Result<(), AppError> {
        let history = self.clone();
        tokio::task::spawn_blocking(move || history.write(jobs))
            .await
            .map_err(|e| AppError::new("JOB_HISTORY_FAILED", e.to_string(), None))?
    }

    fn write(&self, mut jobs: Vec<JobSnapshot>) -> Result<(), AppError> {
        if jobs.len() > MAX_JOBS {
            return Err(error(&self.path, "history exceeds its job limit"));
        }
        // Disk logs hold diagnostics; persisted history needs only a bounded
        // summary, even after 100 verbose jobs.
        for job in &mut jobs {
            job.logs = job
                .logs
                .iter()
                .rev()
                .take(30)
                .rev()
                .map(|line| line.chars().take(500).collect())
                .collect();
        }
        let bytes =
            serde_json::to_vec(&Record { version: 1, jobs }).map_err(|e| error(&self.path, e))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(error(&self.path, "history exceeds its size limit"));
        }
        let temporary = self.path.with_file_name(format!(
            ".jobs-{}-{}.tmp",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| error(&self.path, e))?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            // Same-directory rename replaces the old record only after the new
            // bytes have been flushed. Never truncate the last successful save.
            fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            fs::File::open(self.path.parent().expect("history directory"))?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(cause) = result {
            // This path was reserved with create_new by this exact operation.
            let _ = fs::remove_file(&temporary);
            return Err(error(&self.path, cause));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_and_future_history_is_preserved() {
        let path = std::env::temp_dir().join(format!(
            "jesses-history-invalid-{}-{}",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let file = path.join("jobs.json");
        for bytes in [b"incomplete".as_slice(), br#"{"version":2,"jobs":[]}"#] {
            fs::write(&file, bytes).unwrap();
            assert!(History::open(path.clone()).await.is_err());
            assert_eq!(fs::read(&file).unwrap(), bytes);
        }
        fs::remove_file(file).unwrap();
        fs::remove_file(path.join("jobs.lock")).unwrap();
        fs::remove_dir(path).unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn last_history_owner_unlocks_even_with_an_inherited_descriptor() {
        let path = std::env::temp_dir().join(format!(
            "jesses-history-inherited-{}-{}",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let (history, _) = History::open(path.clone()).await.unwrap();
        history.save(vec![]).await.unwrap();
        // dup and fork preserve the same open file description. A concurrent
        // child can retain this descriptor until exec applies CLOEXEC.
        let inherited = history._lock._file.try_clone().unwrap();
        let writer = history.clone();
        drop(history);
        assert!(History::open(path.clone()).await.is_err());
        drop(writer);
        let (reopened, _) = History::open(path.clone()).await.unwrap();
        drop(inherited);
        assert!(History::open(path.clone()).await.is_err());
        drop(reopened);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn atomic_updates_replace_existing_history() {
        let path = std::env::temp_dir().join(format!(
            "jesses-history-atomic-{}-{}",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let (history, jobs) = History::open(path.clone()).await.unwrap();
        assert!(jobs.is_empty());
        history.save(vec![]).await.unwrap();
        history.save(vec![]).await.unwrap();
        assert!(
            History::open(path.clone()).await.is_err(),
            "another instance cannot overwrite active history"
        );
        drop(history);
        assert!(History::open(path.clone()).await.unwrap().1.is_empty());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 2);
        fs::remove_file(path.join("jobs.json")).unwrap();
        fs::remove_file(path.join("jobs.lock")).unwrap();
        fs::remove_dir(path).unwrap();
    }
}
