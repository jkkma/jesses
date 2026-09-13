use super::CommandSpec;
use std::{
    ffi::OsStr,
    io,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod linux;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod macos;

pub(super) struct OwnedChild {
    pause: Arc<Mutex<Option<i32>>>,
    child: Child,
    pgid: i32,
    terminated: bool,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    tree_exited: bool,
}

pub(super) struct PauseTarget {
    group: Arc<Mutex<Option<i32>>>,
}
#[cfg(target_os = "linux")]
fn confirm_stopped(pgid: i32) -> io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut previous = Vec::new();
    loop {
        let mut members = Vec::new();
        let mut stopped = true;
        for (index, entry) in std::fs::read_dir("/proc")?.enumerate() {
            if index > 100_000 {
                return Err(io::Error::other("Process inventory exceeded its bound."));
            }
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some((_, tail)) = stat.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<_> = tail.split_whitespace().collect();
            if fields.get(2).and_then(|v| v.parse::<i32>().ok()) != Some(pgid) {
                continue;
            }
            if matches!(fields.first(), Some(&"Z" | &"X")) {
                continue;
            }
            members.push(pid);
            stopped &= matches!(fields.first(), Some(&"T" | &"t"));
        }
        members.sort_unstable();
        if stopped && !members.is_empty() && members == previous {
            return Ok(());
        }
        previous = members;
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Could not confirm every worker was stopped.",
            ));
        }
        // Catch a descendant forked just before its parent received SIGSTOP.
        if unsafe { libc::kill(-pgid, libc::SIGSTOP) } == -1 {
            return Err(io::Error::last_os_error());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
impl PauseTarget {
    pub(super) fn set_paused(&self, paused: bool) -> io::Result<()> {
        // The leader is unreaped while this attachment exists, pinning the PGID.
        let group = self
            .group
            .lock()
            .map_err(|_| io::Error::other("Pause state unavailable."))?;
        let pgid = group.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "The process tree has ended.")
        })?;
        let signal = if paused { libc::SIGSTOP } else { libc::SIGCONT };
        if unsafe { libc::kill(-pgid, signal) } == -1 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(target_os = "linux")]
        if paused && let Err(error) = confirm_stopped(pgid) {
            let _ = unsafe { libc::kill(-pgid, libc::SIGCONT) };
            return Err(error);
        }
        Ok(())
    }
}

impl OwnedChild {
    pub(super) fn pause_target(&self) -> io::Result<PauseTarget> {
        Ok(PauseTarget {
            group: self.pause.clone(),
        })
    }
    fn close_pause(&self) {
        *self.pause.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub(super) fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        Self::spawn_with_path(spec, None)
    }

    pub(super) fn spawn_with_path(spec: &CommandSpec, path: Option<&OsStr>) -> io::Result<Self> {
        Self::spawn_with_environment(spec, path.map(super::ChildEnvironment::with_path).as_ref())
    }

    pub(super) fn spawn_with_environment(
        spec: &CommandSpec,
        environment: Option<&super::ChildEnvironment>,
    ) -> io::Result<Self> {
        Self::spawn_with_input(spec, false, None, environment)
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
        environment: Option<&super::ChildEnvironment>,
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
        if let Some(environment) = environment {
            if let Some(path) = &environment.path {
                command.env("PATH", path);
            }
            for (name, value) in &environment.variables {
                if let Some(value) = value {
                    command.env(name, value);
                } else {
                    command.env_remove(name);
                }
            }
        }
        // setpgid happens in the child before exec, closing the spawn/assign race.
        command.process_group(0);
        let child = command.spawn()?;
        let pgid = child.id().expect("new child has a pid") as i32;
        Ok(Self {
            pause: Arc::new(Mutex::new(Some(pgid))),
            child,
            pgid,
            terminated: false,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            tree_exited: false,
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn wait_for_tree_exit(&mut self) -> io::Result<()> {
        // Normal completion is followed by the shared explicit cleanup path.
        // Once the leader has been reaped, never inspect a potentially reused PGID.
        if self.tree_exited {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            // SIGKILL delivery is asynchronous. Keep the leader unreaped so
            // its PID pins this group until every descendant has closed its
            // inherited handles and entered the dead/zombie state.
            #[cfg(target_os = "linux")]
            let live = linux::group_has_live_members(self.pgid)?;
            #[cfg(target_os = "macos")]
            let live = macos::group_has_live_members(self.pgid)?;
            if !live {
                self.tree_exited = true;
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The process group did not finish terminating.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn wait_and_terminate_descendants(&mut self) -> io::Result<ExitStatus> {
        loop {
            if self.has_exited()? {
                self.terminate_tree()?;
                self.close_pause();
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                self.wait_for_tree_exit().await?;
                return self.child.wait().await;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn terminate_and_wait(&mut self) -> io::Result<()> {
        self.terminate_tree()?;
        self.close_pause();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.wait_for_tree_exit().await?;
        self.child.wait().await?;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
        self.close_pause();
    }
}
