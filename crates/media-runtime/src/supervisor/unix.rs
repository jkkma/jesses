use super::CommandSpec;
use std::{
    ffi::OsStr,
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod macos;

pub(super) struct OwnedChild {
    child: Child,
    pgid: i32,
    terminated: bool,
}

impl OwnedChild {
    pub(super) fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        Self::spawn_with_path(spec, None)
    }

    pub(super) fn spawn_with_path(spec: &CommandSpec, path: Option<&OsStr>) -> io::Result<Self> {
        Self::spawn_with_input(spec, false, None, path)
    }

    pub(super) fn spawn_with_stdin(spec: &CommandSpec) -> io::Result<Self> {
        Self::spawn_with_input(spec, true, None, None)
    }

    pub(super) fn spawn_with_stdin_to_file(
        spec: &CommandSpec,
        output: std::fs::File,
    ) -> io::Result<Self> {
        Self::spawn_with_input(spec, true, Some(output), None)
    }

    fn spawn_with_input(
        spec: &CommandSpec,
        pipe_stdin: bool,
        output: Option<std::fs::File>,
        path: Option<&OsStr>,
    ) -> io::Result<Self> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .stdin(if pipe_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(output.map_or_else(Stdio::piped, Stdio::from))
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if let Some(path) = path {
            command.env("PATH", path);
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

    pub(super) fn take_stdin(&mut self) -> ChildStdin {
        self.child.stdin.take().expect("piped stdin")
    }

    pub(super) fn take_output_pipes(&mut self) -> (Option<ChildStdout>, ChildStderr) {
        (
            self.child.stdout.take(),
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
            // Darwin's group signalling skips zombies and can report EPERM
            // for a zombie-only group. Do not suppress real permission errors:
            // prove the leader exited and enumerate all remaining group members.
            #[cfg(target_os = "macos")]
            if error.raw_os_error() == Some(libc::EPERM)
                && self.has_exited()?
                && !macos::group_has_live_members(self.pgid)?
            {
                self.terminated = true;
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        self.terminated = true;
        Ok(())
    }

    fn has_exited(&self) -> io::Result<bool> {
        // Keep platform siginfo_t entirely outside the async state machine:
        // macOS includes raw pointers here, which must never survive an await.
        // WNOWAIT retains the leader, pinning its PID until group cleanup.
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
            return Ok(false);
        }
        Ok(unsafe { info.si_pid() } != 0)
    }

    pub(super) async fn wait_and_terminate_descendants(&mut self) -> io::Result<ExitStatus> {
        loop {
            if self.has_exited()? {
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
