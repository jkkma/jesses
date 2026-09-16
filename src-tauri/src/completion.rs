//! Session-only finish actions. Only a queue observed completing successfully can arm a countdown.
use media_core::{
    AppError, CompletionOptions, CompletionStatus, FinishAction, JobSnapshot, JobState,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
pub struct CompletionMonitor {
    state: Mutex<Monitor>,
    // New work and the final OS dispatch share one admission boundary.
    admission: tokio::sync::Mutex<bool>,
}
#[derive(Default)]
struct Monitor {
    options: CompletionOptions,
    seen: BTreeMap<String, JobState>,
    armed: BTreeSet<String>,
    deadline: Option<Instant>,
    error: Option<String>,
    generation: u64,
}
impl Monitor {
    fn configure(
        &mut self,
        options: CompletionOptions,
        jobs: &[JobSnapshot],
    ) -> Result<(), AppError> {
        let armed: BTreeSet<_> = jobs
            .iter()
            .filter(|j| !j.state.is_terminal())
            .map(|j| j.id.clone())
            .collect();
        if options.finish_action != FinishAction::None && armed.is_empty() {
            return Err(AppError::new(
                "NO_ACTIVE_QUEUE",
                "Start or queue a job before enabling a finish action.",
                None,
            ));
        }
        self.generation = self.generation.wrapping_add(1);
        self.seen = jobs.iter().map(|j| (j.id.clone(), j.state)).collect();
        self.options = options;
        self.armed = if self.options.finish_action == FinishAction::None {
            BTreeSet::new()
        } else {
            armed
        };
        self.deadline = None;
        self.error = None;
        Ok(())
    }
    fn status(&self, now: Instant) -> CompletionStatus {
        CompletionStatus {
            options: self.options.clone(),
            armed_jobs: self.armed.len() as u32,
            seconds_remaining: self
                .deadline
                .map(|d| d.saturating_duration_since(now).as_millis().div_ceil(1000) as u32),
            error: self.error.clone(),
        }
    }
    fn tick(
        &mut self,
        jobs: &[JobSnapshot],
        other_work: bool,
        now: Instant,
    ) -> (Vec<String>, FinishAction) {
        let mut notices = vec![];
        for j in jobs {
            if self.options.finish_action != FinishAction::None && !self.seen.contains_key(&j.id) {
                self.armed.insert(j.id.clone());
            }
            if self.options.notify
                && self.seen.get(&j.id).is_none_or(|s| !s.is_terminal())
                && matches!(j.state, JobState::Succeeded | JobState::Failed)
            {
                notices.push(format!(
                    "{}: {}",
                    if j.state == JobState::Succeeded {
                        "Completed"
                    } else {
                        "Failed"
                    },
                    std::path::Path::new(&j.request.output_path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ));
            }
            self.seen.insert(j.id.clone(), j.state);
        }
        self.seen.retain(|id, _| jobs.iter().any(|j| &j.id == id));
        if self.options.finish_action == FinishAction::None {
            return (notices, FinishAction::None);
        }
        // Any work added before expiry joins the armed queue and restarts the countdown.
        for j in jobs.iter().filter(|j| !j.state.is_terminal()) {
            self.armed.insert(j.id.clone());
            self.deadline = None;
        }
        if self.armed.iter().any(|id| {
            !jobs.iter().any(|j| &j.id == id)
                || jobs
                    .iter()
                    .any(|j| &j.id == id && j.state.is_terminal() && j.state != JobState::Succeeded)
        }) {
            self.options.finish_action = FinishAction::None;
            self.armed.clear();
            self.deadline = None;
            self.error=Some("Finish action disarmed because a job failed, stopped, was canceled or left history.".into());
            return (notices, FinishAction::None);
        }
        if other_work || jobs.iter().any(|j| !j.state.is_terminal()) {
            self.deadline = None;
            return (notices, FinishAction::None);
        }
        if self.armed.is_empty() {
            return (notices, FinishAction::None);
        }
        let deadline = *self.deadline.get_or_insert(now + Duration::from_secs(60));
        if now < deadline {
            return (notices, FinishAction::None);
        }
        (notices, self.options.finish_action)
    }
    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.options.finish_action = FinishAction::None;
        self.deadline = None;
        self.armed.clear();
        self.error = None;
    }
    fn claim(
        &mut self,
        generation: u64,
        jobs: &[JobSnapshot],
        other_work: bool,
        now: Instant,
        dispatch: impl FnOnce(FinishAction) -> Result<(), String>,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        let (_, action) = self.tick(jobs, other_work, now);
        if action == FinishAction::None {
            return false;
        }
        // Caller holds admission and this state lock through dispatch. A cancel
        // wins until this atomic claim; no new work can enter in between.
        let result = dispatch(action);
        self.cancel();
        match result {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
}
impl CompletionMonitor {
    pub async fn admit_work(&self) -> Result<tokio::sync::MutexGuard<'_, bool>, AppError> {
        let admission = self.admission.lock().await;
        if *admission {
            return Err(AppError::new(
                "FINISH_ACTION_IN_PROGRESS",
                "The finish action has already been dispatched.",
                None,
            ));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.deadline = None;
        Ok(admission)
    }
    pub fn configure(
        &self,
        options: CompletionOptions,
        jobs: &[JobSnapshot],
    ) -> Result<CompletionStatus, AppError> {
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner());
        m.configure(options, jobs)?;
        Ok(m.status(Instant::now()))
    }
    pub fn status(&self) -> CompletionStatus {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status(Instant::now())
    }
    pub fn cancel(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
    }
    fn fail(&self, message: String) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(message);
    }
}

pub fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let jobs = app.state::<super::Jobs>().manager.list_jobs().await;
            let monitor = app.state::<CompletionMonitor>();
            let (notices, action, generation) = {
                let mut state = monitor.state.lock().unwrap_or_else(|e| e.into_inner());
                let (notices, action) = state.tick(
                    &jobs,
                    app.state::<super::analysis::AnalysisTasks>().is_running(),
                    Instant::now(),
                );
                (notices, action, state.generation)
            };
            for message in notices {
                if let Err(e) = app
                    .notification()
                    .builder()
                    .title("jesses")
                    .body(message)
                    .show()
                {
                    monitor.fail(format!(
                        "The system could not display the notification: {e}"
                    ));
                }
            }
            if action == FinishAction::None {
                continue;
            }
            let mut admission = monitor.admission.lock().await;
            if *admission {
                continue;
            }
            let fresh_jobs = app.state::<super::Jobs>().manager.list_jobs().await;
            let mut shutdown_child = None;
            let committed = monitor
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .claim(
                    generation,
                    &fresh_jobs,
                    app.state::<super::analysis::AnalysisTasks>().is_running(),
                    Instant::now(),
                    |action| {
                        match action {
                            FinishAction::None => unreachable!(),
                            FinishAction::CloseApp => app.exit(0),
                            FinishAction::Shutdown => shutdown_child = Some(shutdown()?),
                        }
                        Ok(())
                    },
                );
            *admission = committed;
            drop(admission);
            if let Some(child) = shutdown_child {
                let result =
                    tauri::async_runtime::spawn_blocking(move || child.wait_with_output()).await;
                let error = match result {
                    Ok(Ok(output)) if output.status.success() => None,
                    Ok(Ok(output)) => Some(format!(
                        "The system declined shutdown: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )),
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(error) = error {
                    *monitor.admission.lock().await = false;
                    monitor.fail(error);
                }
            } else if committed {
                break;
            }
        }
    });
}

#[cfg(any(windows, target_os = "linux"))]
fn shutdown() -> Result<std::process::Child, String> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let system =
            std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable.")?;
        let mut c = std::process::Command::new(
            std::path::PathBuf::from(system).join("System32/shutdown.exe"),
        );
        c.args(["/s", "/t", "0"]).creation_flags(0x08000000);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = std::process::Command::new("/usr/bin/systemctl");
        c.arg("poweroff");
        c
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn shutdown() -> Result<std::process::Child, String> {
    Err("Shutdown is supported on Windows and Linux.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(state: JobState) -> JobSnapshot {
        serde_json::from_value(serde_json::json!({"id":"a","state":state,"request":{"inputPath":"a.mkv","outputPath":"b.mkv","streamIndices":[0]},"progressSeconds":null,"durationSeconds":null,"logs":[],"error":null})).unwrap()
    }
    #[test]
    fn fast_new_failure_disarms_and_notifications_are_emitted_once() {
        let mut m = Monitor::default();
        m.configure(
            CompletionOptions {
                notify: true,
                finish_action: FinishAction::CloseApp,
            },
            &[job(JobState::Running)],
        )
        .unwrap();
        let mut failed = job(JobState::Failed);
        failed.id = "fast".into();
        let jobs = [job(JobState::Succeeded), failed];
        let (notices, action) = m.tick(&jobs, false, Instant::now());
        assert_eq!(notices.len(), 2);
        assert_eq!(action, FinishAction::None);
        assert_eq!(m.options.finish_action, FinishAction::None);
        assert!(m.tick(&jobs, false, Instant::now()).0.is_empty());
    }
    #[test]
    fn active_utilities_postpone_the_entire_countdown() {
        let mut m = Monitor::default();
        let now = Instant::now();
        m.configure(
            CompletionOptions {
                notify: false,
                finish_action: FinishAction::Shutdown,
            },
            &[job(JobState::Running)],
        )
        .unwrap();
        m.tick(&[job(JobState::Succeeded)], true, now);
        assert!(m.deadline.is_none());
        m.tick(
            &[job(JobState::Succeeded)],
            false,
            now + Duration::from_secs(120),
        );
        assert_eq!(
            m.status(now + Duration::from_secs(120)).seconds_remaining,
            Some(60)
        );
        assert_eq!(
            m.tick(
                &[job(JobState::Succeeded)],
                false,
                now + Duration::from_secs(179)
            )
            .1,
            FinishAction::None
        );
    }
    #[test]
    fn successful_queue_waits_sixty_seconds_and_new_work_resets_it() {
        let mut m = Monitor::default();
        let now = Instant::now();
        m.configure(
            CompletionOptions {
                notify: false,
                finish_action: FinishAction::Shutdown,
            },
            &[job(JobState::Running)],
        )
        .unwrap();
        assert_eq!(
            m.tick(&[job(JobState::Succeeded)], false, now).1,
            FinishAction::None
        );
        assert_eq!(
            m.tick(
                &[job(JobState::Succeeded)],
                false,
                now + Duration::from_secs(59)
            )
            .1,
            FinishAction::None
        );
        assert_eq!(
            m.tick(
                &[job(JobState::Running)],
                false,
                now + Duration::from_secs(60)
            )
            .1,
            FinishAction::None
        );
        assert_eq!(
            m.tick(
                &[job(JobState::Succeeded)],
                false,
                now + Duration::from_secs(61)
            )
            .1,
            FinishAction::None
        );
        assert_eq!(
            m.tick(
                &[job(JobState::Succeeded)],
                false,
                now + Duration::from_secs(121)
            )
            .1,
            FinishAction::Shutdown
        );
        assert_eq!(
            m.status(now + Duration::from_secs(121))
                .options
                .finish_action,
            FinishAction::Shutdown
        );
        assert!(m.claim(
            m.generation,
            &[job(JobState::Succeeded)],
            false,
            now + Duration::from_secs(122),
            |action| {
                assert_eq!(action, FinishAction::Shutdown);
                Ok(())
            }
        ));
        assert_eq!(m.options.finish_action, FinishAction::None);
    }
    #[test]
    fn canceled_queue_disarms_and_historical_success_never_arms() {
        let mut m = Monitor::default();
        let options = CompletionOptions {
            notify: true,
            finish_action: FinishAction::CloseApp,
        };
        assert!(
            m.configure(options.clone(), &[job(JobState::Succeeded)])
                .is_err()
        );
        m.configure(options, &[job(JobState::Running)]).unwrap();
        assert_eq!(
            m.tick(&[job(JobState::Canceled)], false, Instant::now()).1,
            FinishAction::None
        );
        assert_eq!(m.options.finish_action, FinishAction::None);
        assert!(m.armed.is_empty());
    }
    #[tokio::test]
    async fn cancel_or_new_admission_revokes_pending_dispatch() {
        let monitor = CompletionMonitor::default();
        let now = Instant::now();
        for cancel in [true, false] {
            let generation = {
                let mut state = monitor.state.lock().unwrap();
                state
                    .configure(
                        CompletionOptions {
                            notify: false,
                            finish_action: FinishAction::Shutdown,
                        },
                        &[job(JobState::Running)],
                    )
                    .unwrap();
                state.tick(&[job(JobState::Succeeded)], false, now);
                assert_eq!(
                    state
                        .tick(
                            &[job(JobState::Succeeded)],
                            false,
                            now + Duration::from_secs(61)
                        )
                        .1,
                    FinishAction::Shutdown
                );
                state.generation
            };
            if cancel {
                monitor.cancel();
            } else {
                drop(monitor.admit_work().await.unwrap());
            }
            assert!(!monitor.state.lock().unwrap().claim(
                generation,
                &[job(JobState::Succeeded)],
                false,
                now + Duration::from_secs(62),
                |_| panic!("Revoked action dispatched")
            ));
        }
    }
    #[test]
    fn dispatch_rechecks_work_and_reports_os_failure() {
        let mut m = Monitor::default();
        let now = Instant::now();
        m.configure(
            CompletionOptions {
                notify: false,
                finish_action: FinishAction::Shutdown,
            },
            &[job(JobState::Running)],
        )
        .unwrap();
        m.tick(&[job(JobState::Succeeded)], false, now);
        assert!(!m.claim(
            m.generation,
            &[job(JobState::Succeeded)],
            true,
            now + Duration::from_secs(61),
            |_| panic!("Active analysis ignored")
        ));
        assert!(m.deadline.is_none());
        m.tick(
            &[job(JobState::Succeeded)],
            false,
            now + Duration::from_secs(62),
        );
        assert!(!m.claim(
            m.generation,
            &[job(JobState::Succeeded)],
            false,
            now + Duration::from_secs(122),
            |_| Err("dispatch denied".into())
        ));
        assert_eq!(m.options.finish_action, FinishAction::None);
        assert_eq!(m.error.as_deref(), Some("dispatch denied"));
    }
    #[tokio::test]
    async fn committed_dispatch_rejects_new_work() {
        let monitor = CompletionMonitor::default();
        *monitor.admission.lock().await = true;
        assert!(monitor.admit_work().await.is_err());
    }
}
