//! Confirm Darwin's zombie-only process-group edge case without hiding EPERM
//! against live descendants. The group leader remains unreaped during inspection.
use std::{io, mem::size_of};

// Defined by Apple's public bsd/sys/proc_info.h; not exported by libc.
const PROC_PGRP_ONLY: u32 = 2;
const MAX_PIDS: usize = 1_048_576;

pub(super) fn group_has_live_members(pgid: i32) -> io::Result<bool> {
    // proc_listpids includes live and zombie process lists under the kernel's
    // process-list lock. A full buffer might be truncated, so grow and retry.
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c
    let estimate =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, std::ptr::null_mut(), 0) };
    if estimate <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut capacity = (estimate as usize).div_ceil(size_of::<i32>()).max(32);
    loop {
        if capacity > MAX_PIDS {
            return Err(io::Error::other(
                "The process-group membership list exceeded its inspection limit.",
            ));
        }
        let mut pids = vec![0_i32; capacity];
        // libproc returns zero both for an empty list and for errors. Clear errno
        // immediately before the synchronous call to distinguish those outcomes.
        unsafe {
            *libc::__error() = 0;
        }
        let bytes = unsafe {
            libc::proc_listpids(
                PROC_PGRP_ONLY,
                pgid as u32,
                pids.as_mut_ptr().cast(),
                (capacity * size_of::<i32>()) as i32,
            )
        };
        if bytes < 0 || (bytes == 0 && io::Error::last_os_error().raw_os_error() != Some(0)) {
            return Err(io::Error::last_os_error());
        }
        let count = bytes as usize / size_of::<i32>();
        if count >= capacity {
            capacity *= 2;
            continue;
        }
        for &pid in &pids[..count] {
            let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
            let bytes = unsafe {
                // Nonzero arg includes zombies. SHORTBSDINFO does not require
                // same-user privileges, so live elevated descendants are visible.
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDT_SHORTBSDINFO,
                    1,
                    (&mut info as *mut libc::proc_bsdshortinfo).cast(),
                    size_of::<libc::proc_bsdshortinfo>() as i32,
                )
            };
            if bytes != size_of::<libc::proc_bsdshortinfo>() as i32 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    continue;
                }
                return Err(error);
            }
            // A descendant can disappear between listing and inspection. Never
            // classify a reused PID from another group as one of our processes.
            if info.pbsi_pgid == pgid as u32 && info.pbsi_status != libc::SZOMB {
                return Ok(true);
            }
        }
        return Ok(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_live_group_is_never_classified_as_zombie_only() {
        assert!(group_has_live_members(unsafe { libc::getpgrp() }).unwrap());
    }

    #[tokio::test]
    async fn exited_unreaped_leader_is_confirmed_zombie_only_before_cleanup() {
        let spec = crate::supervisor::CommandSpec {
            executable: "/bin/true".into(),
            args: Vec::new(),
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
