//! Live pause applies only to an attached, owned process tree.
use super::platform;
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
struct Active {
    target: platform::PauseTarget,
    started: Instant,
    paused_at: Option<Instant>,
    paused_for: Duration,
}
#[derive(Clone)]
pub struct PauseControl {
    active: Arc<Mutex<Option<Active>>>,
    changed: watch::Sender<u64>,
}
impl Default for PauseControl {
    fn default() -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            active: Arc::new(Mutex::new(None)),
            changed,
        }
    }
}
impl PauseControl {
    pub fn set_paused(&self, paused: bool) -> io::Result<()> {
        let mut state = self
            .active
            .lock()
            .map_err(|_| io::Error::other("Pause state unavailable."))?;
        let active = state.as_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The av1an process is not running. Pause is available during chunk encoding.",
            )
        })?;
        if paused == active.paused_at.is_some() {
            return Ok(());
        }
        active.target.set_paused(paused)?;
        if paused {
            active.paused_at = Some(Instant::now());
        } else if let Some(start) = active.paused_at.take() {
            active.paused_for += start.elapsed();
        }
        self.changed
            .send_modify(|value| *value = value.wrapping_add(1));
        Ok(())
    }
    pub(super) fn attach(&self, child: &platform::OwnedChild) -> io::Result<Attachment> {
        let mut state = self
            .active
            .lock()
            .map_err(|_| io::Error::other("Pause state unavailable."))?;
        if state.is_some() {
            return Err(io::Error::other("A process tree is already attached."));
        }
        *state = Some(Active {
            target: child.pause_target()?,
            started: Instant::now(),
            paused_at: None,
            paused_for: Duration::ZERO,
        });
        self.changed
            .send_modify(|value| *value = value.wrapping_add(1));
        Ok(Attachment(self.clone()))
    }
    pub(super) async fn timeout(&self, limit: Duration) {
        let mut changed = self.changed.subscribe();
        loop {
            let remaining = {
                let state = self.active.lock().unwrap_or_else(|e| e.into_inner());
                state.as_ref().map(|active| {
                    if active.paused_at.is_some() {
                        None
                    } else {
                        let remaining = limit.saturating_sub(
                            active.started.elapsed().saturating_sub(active.paused_for),
                        );
                        Some(remaining)
                    }
                })
            };
            match remaining {
                Some(Some(remaining)) => {
                    if remaining.is_zero() {
                        return;
                    }
                    // Recheck the clock after waking: a pause can race the timer.
                    tokio::select! {_ = tokio::time::sleep(remaining)=>{},_=changed.changed()=>{}}
                }
                _ => {
                    let _ = changed.changed().await;
                }
            }
        }
    }
}
pub(super) struct Attachment(PauseControl);
impl Drop for Attachment {
    fn drop(&mut self) {
        let mut state = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        *state = None;
        self.0.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
}
