//! Confirm group exit before reaping the leader that pins the owned PGID.
use std::{fs::File, io, io::Read};

const MAX_PROCESSES: usize = 100_000;
const MAX_STAT_BYTES: u64 = 8192;

pub(super) fn group_has_live_members(pgid: i32) -> io::Result<bool> {
    if pgid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The owned process group must have a positive ID.",
        ));
    }
    for (index, entry) in std::fs::read_dir("/proc")?.enumerate() {
        if index >= MAX_PROCESSES {
            return Err(io::Error::other(
                "The process inventory exceeded its inspection limit.",
            ));
        }
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let read_stat = || -> io::Result<Vec<u8>> {
            let mut stat = Vec::with_capacity(512);
            File::open(entry.path().join("stat"))?
                .take(MAX_STAT_BYTES + 1)
                .read_to_end(&mut stat)?;
            if stat.len() as u64 > MAX_STAT_BYTES {
                return Err(invalid_stat(
                    "Process status exceeded its inspection limit.",
                ));
            }
            Ok(stat)
        };
        let stat = match read_stat() {
            Ok(stat) => stat,
            // Exiting processes can disappear between listing and reading.
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if is_live_group_member(&stat, pgid)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn invalid_stat(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn is_live_group_member(stat: &[u8], pgid: i32) -> io::Result<bool> {
    // comm may contain spaces, parentheses, newlines, or non-UTF8 bytes. All
    // remaining fields are numeric apart from the single-byte process state.
    let end = stat
        .iter()
        .rposition(|&byte| byte == b')')
        .ok_or_else(|| invalid_stat("Process status has no command delimiter."))?;
    let mut fields = stat[end + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty());
    let state = fields.next();
    let group = fields
        .nth(1)
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse::<i32>().ok())
        .ok_or_else(|| invalid_stat("Process status has no valid process group ID."))?;
    if group != pgid {
        return Ok(false);
    }
    let state = match state {
        Some([state]) if b"RSDZTtWXxKPI".contains(state) => *state,
        _ => return Err(invalid_stat("Owned process status has no valid state.")),
    };
    // After pgrp (field 5), num_threads (field 20) is the fifteenth field.
    let threads = fields
        .nth(14)
        .and_then(|field| std::str::from_utf8(field).ok())
        .and_then(|field| field.parse::<u32>().ok())
        .ok_or_else(|| invalid_stat("Owned process status has no valid thread count."))?;
    // A zombie main thread can retain live worker threads. Linux keeps those
    // threads in signal->nr_threads until release_task removes each one.
    // An ordinary unreaped zombie has one thread and no executable work left.
    Ok(!matches!(state, b'Z' | b'X' | b'x') || threads > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(name: &[u8], state: &str, group: i32, threads: &str) -> Vec<u8> {
        let mut bytes = b"42 (".to_vec();
        bytes.extend_from_slice(name);
        // state, ppid, pgrp, then fields 6 through 19, then num_threads.
        bytes.extend_from_slice(format!(") {state} 1 {group} ").as_bytes());
        bytes.extend_from_slice("0 ".repeat(14).as_bytes());
        bytes.extend_from_slice(threads.as_bytes());
        bytes
    }

    #[test]
    fn parses_unescaped_process_names_and_checks_the_exact_group() {
        let value = stat(b"a ) (\n\xff name)", "S", 4242, "3");
        assert!(is_live_group_member(&value, 4242).unwrap());
        assert!(!is_live_group_member(&value, 4243).unwrap());
    }

    #[test]
    fn stopped_workers_remain_live_and_zombies_require_all_threads_to_exit() {
        for state in ["R", "S", "D", "T", "t", "I"] {
            assert!(is_live_group_member(&stat(b"tool", state, 42, "1"), 42).unwrap());
        }
        for state in ["Z", "X", "x"] {
            assert!(!is_live_group_member(&stat(b"tool", state, 42, "1"), 42).unwrap());
            assert!(is_live_group_member(&stat(b"tool", state, 42, "2"), 42).unwrap());
        }
    }

    #[test]
    fn malformed_status_cannot_confirm_owned_process_exit() {
        for value in [
            b"42 tool Z 1 42".to_vec(),
            b"42 (tool) Z 1 unknown".to_vec(),
            stat(b"tool", "invalid", 42, "1"),
            stat(b"tool", "Z", 42, "unknown"),
            b"42 (tool) Z 1 42".to_vec(),
        ] {
            assert_eq!(
                is_live_group_member(&value, 42).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        // The unrelated process's state is not used for our cleanup decision.
        assert!(!is_live_group_member(&stat(b"other", "invalid", 43, "?"), 42).unwrap());
    }

    #[test]
    fn current_live_group_is_never_classified_as_exited() {
        assert!(group_has_live_members(unsafe { libc::getpgrp() }).unwrap());
        assert_eq!(
            group_has_live_members(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[tokio::test]
    async fn exited_unreaped_leader_is_not_live_before_cleanup() {
        let spec = crate::supervisor::CommandSpec {
            executable: std::env::current_exe().unwrap(),
            args: vec!["--exact".into(), "__supervisor_empty_selection__".into()],
            cwd: None,
        };
        let mut child = super::super::OwnedChild::spawn(&spec).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !child.has_exited().unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!group_has_live_members(child.pgid).unwrap());
        child.terminate_and_wait().await.unwrap();
    }
}
