//! Pause every process tree belonging to one operation, including both pipe stages.
use super::platform;
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;

struct State {
    targets: BTreeMap<u64, platform::PauseTarget>,
    sequence: u64,
    scopes: u32,
    origin: Instant,
    paused_at: Option<Instant>,
    paused_for: Duration,
}
impl Default for State {
    fn default() -> Self {
        Self {
            targets: BTreeMap::new(),
            sequence: 0,
            scopes: 0,
            origin: Instant::now(),
            paused_at: None,
            paused_for: Duration::ZERO,
        }
    }
}
#[derive(Clone)]
pub struct PauseControl {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
}
impl Default for PauseControl {
    fn default() -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            state: Arc::new(Mutex::new(State::default())),
            changed,
        }
    }
}
tokio::task_local! { static OPERATION_PAUSE: PauseControl; }

/// Scope process launches to the operation's own control. No unrelated operation is affected.
pub async fn with_pause<T>(control: PauseControl, future: impl Future<Output = T>) -> T {
    {
        control
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scopes += 1;
    }
    let _scope = Scope(control.clone());
    OPERATION_PAUSE.scope(control, future).await
}
pub(crate) fn scoped_pause() -> Option<PauseControl> {
    OPERATION_PAUSE.try_with(Clone::clone).ok()
}
struct Scope(PauseControl);
impl Drop for Scope {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.scopes = state.scopes.saturating_sub(1);
        if state.scopes == 0
            && state.targets.is_empty()
            && let Some(start) = state.paused_at.take()
        {
            state.paused_for += start.elapsed();
        }
        self.0.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
}
impl PauseControl {
    pub(crate) fn active_elapsed(&self) -> Duration {
        self.running_clock().0
    }
    pub fn is_paused(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paused_at
            .is_some()
    }
    /// Native work between child processes must also wait before publication.
    pub(crate) async fn wait_until_resumed(
        &self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), super::SupervisorError> {
        let mut changed = self.changed.subscribe();
        let mut canceled = cancel.clone();
        loop {
            if *canceled.borrow() {
                return Err(super::SupervisorError::Cancelled);
            }
            if !self.is_paused() {
                return Ok(());
            }
            tokio::select! {
                biased;
                change = canceled.changed() => {
                    if change.is_err() || *canceled.borrow() { return Err(super::SupervisorError::Cancelled); }
                }
                _ = changed.changed() => {}
            }
        }
    }
    pub fn set_paused(&self, paused: bool) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("Pause state unavailable."))?;
        if state.targets.is_empty() && state.scopes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The operation is no longer running.",
            ));
        }
        let previous = state.paused_at.is_some();
        if paused == previous {
            return Ok(());
        }
        let mut changed = Vec::new();
        for (id, target) in &state.targets {
            if let Err(error) = target.set_paused(paused) {
                for id in changed {
                    if let Some(target) = state.targets.get(&id) {
                        let _ = target.set_paused(previous);
                    }
                }
                return Err(error);
            }
            changed.push(*id);
        }
        if paused {
            state.paused_at = Some(Instant::now());
        } else if let Some(start) = state.paused_at.take() {
            state.paused_for += start.elapsed();
        }
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }
    pub(super) fn attach(&self, child: &platform::OwnedChild) -> io::Result<Attachment> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("Pause state unavailable."))?;
        let target = child.pause_target()?;
        if state.paused_at.is_some() {
            target.set_paused(true)?;
        }
        let id = state.sequence;
        state.sequence = state.sequence.wrapping_add(1);
        state.targets.insert(id, target);
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(Attachment(self.clone(), id))
    }
    fn running_clock(&self) -> (Duration, bool) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current_pause = state
            .paused_at
            .map_or(Duration::ZERO, |start| start.elapsed());
        (
            state
                .origin
                .elapsed()
                .saturating_sub(state.paused_for + current_pause),
            state.paused_at.is_some(),
        )
    }
    pub(super) async fn timeout(&self, limit: Duration) {
        let mut changed = self.changed.subscribe();
        let (start, _) = self.running_clock();
        loop {
            let (now, paused) = self.running_clock();
            let remaining = limit.saturating_sub(now.saturating_sub(start));
            if !paused && remaining.is_zero() {
                return;
            }
            if paused {
                let _ = changed.changed().await;
            } else {
                tokio::select! { _ = tokio::time::sleep(remaining) => {}, _ = changed.changed() => {} }
            }
        }
    }
}
pub(super) struct Attachment(PauseControl, u64);
impl Drop for Attachment {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.targets.remove(&self.1);
        if state.targets.is_empty()
            && state.scopes == 0
            && let Some(start) = state.paused_at.take()
        {
            state.paused_for += start.elapsed();
        }
        self.0.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
}
