use super::CommandSpec;
use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

pub(super) struct OwnedChild {
    child: Child,
    pgid: i32,
    terminated: bool,
}

impl OwnedChild {
    pub(super) fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        // setpgid happens in the child before exec, closing the spawn/assign race.
        command.process_group(0);
        let child = command.spawn()?;
        let pgid = child.id().expect("new child has a pid") as i32;
        Ok(Self {
            child,
            pgid,
            terminated: false,
        })
    }

    pub(super) fn take_pipes(&mut self) -> (ChildStdout, ChildStderr) {
        (
            self.child.stdout.take().expect("piped stdout"),
            self.child.stderr.take().expect("piped stderr"),
        )
    }

    fn terminate_tree(&mut self) -> io::Result<()> {
        if self.terminated {
            return Ok(());
        }
        // SAFETY: pgid belongs to the newly created process group, never ours.
        if unsafe { libc::kill(-self.pgid, libc::SIGKILL) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        self.terminated = true;
        Ok(())
    }

    pub(super) async fn wait_and_terminate_descendants(&mut self) -> io::Result<ExitStatus> {
        loop {
            // Observe exit without reaping: retaining the leader pins its PID and
            // prevents a reused process-group ID from being signalled by cleanup.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pgid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            } else if unsafe { info.si_pid() } != 0 {
                self.terminate_tree()?;
                return self.child.wait().await;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn terminate_and_wait(&mut self) -> io::Result<()> {
        self.terminate_tree()?;
        self.child.wait().await?;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
    }
}
