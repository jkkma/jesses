//! Cancellation is registered before expensive inspection can start. Canceling
//! an unused ticket also prevents a delayed command from starting afterwards.
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use media_core::AppError;
use tokio::sync::watch;

#[derive(Default)]
pub struct AnalysisTasks {
    sequence: AtomicU64,
    tasks: Mutex<HashMap<String, Task>>,
    closing: AtomicBool,
}

struct Task {
    cancel: watch::Sender<bool>,
    started: bool,
    pause: media_runtime::supervisor::PauseControl,
    created: Instant,
    kind: TicketKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TicketKind {
    Analysis,
    PendingImport,
    Import,
}

impl Task {
    fn expired(&self) -> bool {
        self.kind != TicketKind::Import && self.created.elapsed() >= Duration::from_secs(30)
    }
}

pub struct RunningAnalysis<'a> {
    owner: &'a AnalysisTasks,
    id: String,
    pub cancel: watch::Receiver<bool>,
    pub pause: media_runtime::supervisor::PauseControl,
}

fn failure(code: &str, message: &str) -> AppError {
    AppError::new(code, message, None)
}

impl AnalysisTasks {
    pub fn is_running(&self) -> bool {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.retain(|_, task| task.started || !task.expired());
        !tasks.is_empty()
    }

    pub fn begin(&self) -> Result<String, AppError> {
        self.reserve(TicketKind::Analysis)
    }

    pub fn begin_import(&self) -> Result<String, AppError> {
        self.reserve(TicketKind::PendingImport)
    }

    pub fn claim_import(&self, id: &str) -> Result<(), AppError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let Some(task) = tasks
            .get_mut(id)
            .filter(|task| task.kind == TicketKind::PendingImport && !task.expired())
        else {
            return Err(failure(
                "ANALYSIS_CANCELLED",
                "The import reservation was canceled or expired.",
            ));
        };
        if self.closing.load(Ordering::Acquire) {
            return Err(failure("ANALYSIS_CANCELLED", "The application is closing."));
        }
        // A page can disappear before receiving the begin reply. Only a page
        // that acknowledges its ticket can keep the reservation beyond expiry.
        task.kind = TicketKind::Import;
        Ok(())
    }

    fn reserve(&self, kind: TicketKind) -> Result<String, AppError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return Err(failure("ANALYSIS_CANCELLED", "The application is closing."));
        }
        tasks.retain(|_, task| task.started || !task.expired());
        if tasks.len() >= 4 {
            return Err(failure(
                "ANALYSIS_BUSY",
                "Wait for an active source inspection to finish.",
            ));
        }
        let id = format!(
            "{}-{}",
            std::process::id(),
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let (cancel, _) = watch::channel(false);
        tasks.insert(
            id.clone(),
            Task {
                cancel,
                started: false,
                pause: Default::default(),
                created: Instant::now(),
                kind,
            },
        );
        Ok(id)
    }

    pub fn run(&self, id: &str) -> Result<RunningAnalysis<'_>, AppError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        let task = tasks.get_mut(id).ok_or_else(|| {
            failure(
                "ANALYSIS_CANCELLED",
                "Source inspection was canceled or expired.",
            )
        })?;
        if task.started {
            return Err(failure(
                "ANALYSIS_ALREADY_STARTED",
                "This source inspection has already started.",
            ));
        }
        if task.expired() {
            tasks.remove(id);
            return Err(failure(
                "ANALYSIS_CANCELLED",
                "Source inspection expired before it started.",
            ));
        }
        if task.kind == TicketKind::PendingImport {
            return Err(failure(
                "ANALYSIS_NOT_READY",
                "The import reservation has not been claimed.",
            ));
        }
        task.started = true;
        Ok(RunningAnalysis {
            owner: self,
            id: id.to_owned(),
            cancel: task.cancel.subscribe(),
            pause: task.pause.clone(),
        })
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> Result<(), AppError> {
        let control = {
            let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            let task = tasks
                .get(id)
                .filter(|task| task.started && !*task.cancel.borrow())
                .ok_or_else(|| {
                    failure("ANALYSIS_NOT_RUNNING", "This utility is no longer running.")
                })?;
            task.pause.clone()
        };
        control
            .set_paused(paused)
            .map_err(|cause| AppError::new("PAUSE_FAILED", cause.to_string(), None))
    }

    pub fn cancel(&self, id: &str) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = tasks.get(id) {
            task.cancel.send_replace(true);
            if !task.started {
                tasks.remove(id);
            }
        }
    }

    pub fn cancel_all(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for task in tasks.values() {
            task.cancel.send_replace(true);
        }
        tasks.retain(|_, task| task.started);
    }

    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        self.cancel_all();
        loop {
            if self
                .tasks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for RunningAnalysis<'_> {
    fn drop(&mut self) {
        let mut tasks = self.owner.tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = tasks.get_mut(&self.id)
            && task.kind == TicketKind::Import
            && !*task.cancel.borrow()
        {
            task.started = false;
        } else {
            tasks.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_tickets_defer_completion_until_canceled_or_finished() {
        let tasks = AnalysisTasks::default();
        assert!(!tasks.is_running());
        let id = tasks.begin().unwrap();
        assert!(tasks.is_running());
        tasks.cancel(&id);
        assert!(!tasks.is_running());
        let id = tasks.begin().unwrap();
        let running = tasks.run(&id).unwrap();
        assert!(tasks.is_running());
        drop(running);
        assert!(!tasks.is_running());
    }

    #[test]
    fn cancel_before_command_prevents_late_start() {
        let tasks = AnalysisTasks::default();
        let id = tasks.begin().unwrap();
        tasks.cancel(&id);
        assert!(tasks.run(&id).is_err());
    }

    #[test]
    fn cancel_is_scoped_and_running_ticket_is_single_use() {
        let tasks = AnalysisTasks::default();
        let a = tasks.begin().unwrap();
        let b = tasks.begin().unwrap();
        let first = tasks.run(&a).unwrap();
        let second = tasks.run(&b).unwrap();
        assert!(tasks.run(&a).is_err());
        tasks.cancel(&a);
        assert!(*first.cancel.borrow());
        assert!(!*second.cancel.borrow());
        drop(first);
        assert!(tasks.run(&a).is_err());
    }

    #[test]
    fn import_ticket_stays_active_across_idle_gaps_and_releases_after_probe_cleanup() {
        let tasks = AnalysisTasks::default();
        let id = tasks.begin_import().unwrap();
        assert!(tasks.run(&id).is_err());
        tasks.claim_import(&id).unwrap();
        tasks.tasks.lock().unwrap().get_mut(&id).unwrap().created =
            Instant::now() - Duration::from_secs(31);
        assert!(tasks.is_running());
        let first = tasks.run(&id).unwrap();
        assert!(tasks.run(&id).is_err());
        drop(first);
        assert!(tasks.is_running(), "The import is active between probes");
        let second = tasks.run(&id).unwrap();
        tasks.cancel(&id);
        assert!(*second.cancel.borrow());
        assert!(
            tasks.is_running(),
            "The canceled probe still owns its process"
        );
        drop(second);
        assert!(!tasks.is_running());
        assert!(tasks.run(&id).is_err());
    }

    #[tokio::test]
    async fn lost_import_reply_expires_after_navigation_during_admission() {
        let tasks = AnalysisTasks::default();
        let monitor = crate::completion::CompletionMonitor::default();
        let held_admission = monitor.admit_work().await.unwrap();
        let pending = async {
            let _admission = monitor.admit_work().await.unwrap();
            tasks.begin_import().unwrap()
        };
        tokio::pin!(pending);
        tokio::select! {
            biased;
            _ = &mut pending => panic!("Import ignored the completion admission lock"),
            _ = std::future::ready(()) => {}
        }

        // A page load can cancel existing tickets while an old IPC reply is
        // still waiting for admission and cannot reach its original document.
        tasks.cancel_all();
        drop(held_admission);
        let id = pending.await;
        tasks.tasks.lock().unwrap().get_mut(&id).unwrap().created =
            Instant::now() - Duration::from_secs(31);
        assert!(
            !tasks.is_running(),
            "A lost import reply kept shutdown blocked"
        );
        assert!(tasks.tasks.lock().unwrap().is_empty());
        assert!(tasks.claim_import(&id).is_err());
    }

    #[tokio::test]
    async fn shutdown_cancels_and_waits_for_owned_work() {
        let tasks = AnalysisTasks::default();
        let id = tasks.begin().unwrap();
        let running = tasks.run(&id).unwrap();
        tasks.cancel_all();
        assert!(*running.cancel.borrow());
        drop(running);
        tasks.shutdown().await;
        assert!(tasks.tasks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn window_close_rejects_new_probes_while_waiting_for_active_probe_cleanup() {
        let tasks = AnalysisTasks::default();
        let id = tasks.begin().unwrap();
        let running = tasks.run(&id).unwrap();
        let closing = tasks.shutdown();
        tokio::pin!(closing);
        tokio::select! {
            biased;
            _ = &mut closing => panic!("Window close skipped active probe cleanup"),
            _ = std::future::ready(()) => {}
        }
        assert!(*running.cancel.borrow());
        assert!(
            tasks.begin().is_err(),
            "A late probe entered after window close"
        );
        drop(running);
        closing.await;
        assert!(!tasks.is_running());
    }
}
