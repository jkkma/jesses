//! Cancellation is registered before expensive inspection can start. Canceling
//! an unused ticket also prevents a delayed command from starting afterwards.
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use media_core::AppError;
use tokio::sync::watch;

#[derive(Default)]
pub struct AnalysisTasks {
    sequence: AtomicU64,
    tasks: Mutex<HashMap<String, Task>>,
}

struct Task {
    cancel: watch::Sender<bool>,
    started: bool,
    created: Instant,
}

pub struct RunningAnalysis<'a> {
    owner: &'a AnalysisTasks,
    id: String,
    pub cancel: watch::Receiver<bool>,
}

fn failure(code: &str, message: &str) -> AppError {
    AppError::new(code, message, None)
}

impl AnalysisTasks {
    pub fn begin(&self) -> Result<String, AppError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.retain(|_, task| task.started || task.created.elapsed() < Duration::from_secs(30));
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
                created: Instant::now(),
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
        if task.created.elapsed() >= Duration::from_secs(30) {
            tasks.remove(id);
            return Err(failure(
                "ANALYSIS_CANCELLED",
                "Source inspection expired before it started.",
            ));
        }
        task.started = true;
        Ok(RunningAnalysis {
            owner: self,
            id: id.to_owned(),
            cancel: task.cancel.subscribe(),
        })
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
        self.owner
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
