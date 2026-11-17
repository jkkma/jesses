//! Job log files share the job's id as their file-name stem (`<id>.log`,
//! `<id>.mux.log`, `<id>.av1an.log`, each with an optional `.1` archive).
//! They are removed together with the history entry that owns them, and logs
//! that no history entry owns are removed after a grace period at startup.
//! Every removal is best effort: a log that cannot be removed is left alone.
use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, SystemTime},
};

/// Logs without a history entry are kept this long before startup removes them.
pub(super) const ORPHAN_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Ids are generated as `<pid>-<nanoseconds>-<sequence>`. Anything else in the
/// log folder is not a job log and is never touched.
fn is_job_id(value: &str) -> bool {
    let mut parts = value.split('-');
    let valid = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    };
    valid(parts.next()) && valid(parts.next()) && valid(parts.next()) && parts.next().is_none()
}

/// The owning job id of a log file name, when the name is a job log.
fn owner(name: &str) -> Option<&str> {
    let (stem, rest) = name.split_once('.')?;
    let log = rest.strip_suffix(".1").unwrap_or(rest);
    (is_job_id(stem) && (log == "log" || log.ends_with(".log"))).then_some(stem)
}

/// Removes every log file of the given jobs.
pub(super) fn remove(log_dir: &Path, ids: &HashSet<String>) {
    if ids.is_empty() {
        return;
    }
    sweep(log_dir, |id, _| ids.contains(id));
}

/// Removes job logs that belong to no known job and were last written before
/// `now - ORPHAN_AGE`.
pub(super) fn remove_orphans(log_dir: &Path, known: &HashSet<String>, now: SystemTime) {
    let Some(cutoff) = now.checked_sub(ORPHAN_AGE) else {
        return;
    };
    sweep(log_dir, |id, path| {
        !known.contains(id)
            && std::fs::symlink_metadata(path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified < cutoff)
    });
}

fn sweep(log_dir: &Path, remove: impl Fn(&str, &Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(owner) else {
            continue;
        };
        let path = entry.path();
        if remove(id, &path) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jesses-log-retention-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn only_job_log_names_have_an_owner() {
        assert_eq!(owner("12-345-6.log"), Some("12-345-6"));
        assert_eq!(owner("12-345-6.log.1"), Some("12-345-6"));
        assert_eq!(owner("12-345-6.mux.log"), Some("12-345-6"));
        assert_eq!(
            owner("12-345-6.source-normalization.log.1"),
            Some("12-345-6")
        );
        assert_eq!(owner("12-345-6.json"), None);
        assert_eq!(owner("12-345.log"), None);
        assert_eq!(owner("12-345-6-7.log"), None);
        assert_eq!(owner("a-345-6.log"), None);
        assert_eq!(owner("app.log"), None);
        assert_eq!(owner("12-345-6"), None);
    }

    #[test]
    fn removing_a_job_takes_all_of_its_logs_and_nothing_else() {
        let dir = fixture();
        for name in [
            "1-10-1.log",
            "1-10-1.log.1",
            "1-10-1.mux.log",
            "1-10-1.av1an.log.1",
            "1-10-2.log",
            "1-10-10.log",
            "notes.txt",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.join("1-10-1.dir.log")).unwrap();
        remove(&dir, &HashSet::from(["1-10-1".to_owned()]));
        assert_eq!(
            names(&dir),
            ["1-10-1.dir.log", "1-10-10.log", "1-10-2.log", "notes.txt"]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_removes_only_old_logs_without_a_history_entry() {
        let dir = fixture();
        for name in ["1-10-1.log", "1-10-2.log", "1-10-2.mux.log", "other.log"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let known = HashSet::from(["1-10-1".to_owned()]);
        // Freshly written orphans are kept.
        remove_orphans(&dir, &known, SystemTime::now());
        assert_eq!(names(&dir).len(), 4);
        // Once they are older than the grace period they go; owned logs and
        // files that are not job logs stay.
        remove_orphans(
            &dir,
            &known,
            SystemTime::now() + ORPHAN_AGE + Duration::from_secs(60),
        );
        assert_eq!(names(&dir), ["1-10-1.log", "other.log"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
