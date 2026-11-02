//! Bounded, operation-scoped utility status. This never carries tool output or paths.
use super::*;
use media_core::UtilityProgress;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Context {
    state: Arc<Mutex<State>>,
    sink: Arc<dyn Fn(UtilityProgress) + Send + Sync>,
    pause: Option<crate::supervisor::PauseControl>,
}
struct State {
    origin: Instant,
    clock_origin: Duration,
    value: UtilityProgress,
    pending: Vec<u8>,
    last_emit: Instant,
    estimate: Option<f64>,
    phase_started: f64,
}
tokio::task_local! { static CURRENT: Context; }

/// Progress callbacks are synchronous and bounded; callers should enqueue or emit only.
pub async fn with_utility_progress<T>(
    sink: impl Fn(UtilityProgress) + Send + Sync + 'static,
    future: impl Future<Output = T>,
) -> T {
    let pause = crate::supervisor::scoped_pause();
    let context = Context {
        state: Arc::new(Mutex::new(State {
            origin: Instant::now(),
            clock_origin: pause
                .as_ref()
                .map_or(Duration::ZERO, |pause| pause.active_elapsed()),
            value: UtilityProgress {
                row_index: None,
                row_count: 1,
                completed_rows: 0,
                status: "running".into(),
                phase: "preparing".into(),
                message: "Preparing utility".into(),
                elapsed_seconds: 0.0,
                progress_seconds: None,
                duration_seconds: None,
                estimated_remaining_seconds: None,
            },
            pending: Vec::new(),
            last_emit: Instant::now(),
            estimate: None,
            phase_started: 0.0,
        })),
        sink: Arc::new(sink),
        pause,
    };
    let observer = context.clone();
    let heartbeat = context.clone();
    let monitored = async move {
        tokio::pin!(future);
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.tick().await;
        loop {
            tokio::select! {
                result = &mut future => return result,
                _ = timer.tick() => heartbeat.update(|_| {}),
            }
        }
    };
    CURRENT
        .scope(
            context,
            crate::supervisor::with_capture_observer(
                Arc::new(move |stderr, bytes| observer.observe(stderr, bytes)),
                monitored,
            ),
        )
        .await
}
impl Context {
    fn update(&self, change: impl FnOnce(&mut State)) {
        let (value, emit) = {
            let mut state = self.state.lock().unwrap_or_else(|cause| cause.into_inner());
            let previous_row = state.value.row_index;
            let previous_status = state.value.status.clone();
            let first = state.value.elapsed_seconds == 0.0;
            change(&mut state);
            state.value.elapsed_seconds = self
                .pause
                .as_ref()
                .map_or_else(
                    || state.origin.elapsed(),
                    |pause| pause.active_elapsed().saturating_sub(state.clock_origin),
                )
                .as_secs_f64();
            state.value.estimated_remaining_seconds = state
                .estimate
                .map(|total| (total - state.value.elapsed_seconds).max(0.0));
            // Keep running updates at ten per second. Row transitions and terminal
            // status are always delivered, including very short or failed files.
            let emit = first
                || previous_row != state.value.row_index
                || previous_status != state.value.status
                || state.value.status != "running"
                || state.last_emit.elapsed() >= Duration::from_millis(100);
            if emit {
                state.last_emit = Instant::now();
            }
            (state.value.clone(), emit)
        };
        if emit {
            (self.sink)(value);
        }
    }
    fn observe(&self, stderr: bool, bytes: &[u8]) {
        if !stderr {
            return;
        }
        let mut updates = Vec::new();
        {
            let mut state = self.state.lock().unwrap_or_else(|cause| cause.into_inner());
            for &byte in bytes {
                if byte == b'\n' || byte == b'\r' {
                    if let Ok(line) = std::str::from_utf8(&state.pending)
                        && let Some(value) = line
                            .strip_prefix("out_time_us=")
                            .and_then(|v| v.parse::<u64>().ok())
                    {
                        updates.push(value as f64 / 1_000_000.0);
                    }
                    state.pending.clear();
                } else if state.pending.len() < 1024 {
                    state.pending.push(byte);
                }
            }
            if state.last_emit.elapsed() < Duration::from_millis(100) {
                return;
            }
        }
        if let Some(seconds) = updates.last() {
            self.update(|state| {
                state.value.progress_seconds = Some(*seconds);
                if let Some(duration) = state
                    .value
                    .duration_seconds
                    .filter(|duration| *duration > 0.0)
                {
                    let elapsed = self
                        .pause
                        .as_ref()
                        .map_or_else(
                            || state.origin.elapsed(),
                            |pause| pause.active_elapsed().saturating_sub(state.clock_origin),
                        )
                        .as_secs_f64();
                    let phase_elapsed = (elapsed - state.phase_started).max(0.0);
                    if *seconds > 0.1 {
                        state.estimate =
                            Some(elapsed + phase_elapsed * (duration / seconds - 1.0).max(0.0));
                    }
                }
            });
        }
    }
}
pub(super) fn active() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}
pub(super) fn row(index: usize, count: usize) {
    let _ = CURRENT.try_with(|context| {
        context.update(|state| {
            state.value.row_index = Some(index as u32);
            state.value.row_count = count as u32;
            state.value.status = "running".into();
            state.value.phase = "preparing".into();
            state.value.message = format!("Preparing batch row {} of {count}", index + 1);
            state.value.progress_seconds = None;
            state.value.duration_seconds = None;
            state.estimate = None;
        })
    });
}
pub(super) fn phase(phase: &str, message: &str) {
    let _ = CURRENT.try_with(|context| {
        context.update(|state| {
            state.value.phase = phase.into();
            state.value.message = message.into();
            state.value.status = "running".into();
            state.value.progress_seconds = None;
            state.phase_started = state.value.elapsed_seconds;
        })
    });
}
pub(super) fn terminal(status: &str, message: &str) {
    let _ = CURRENT.try_with(|context| {
        context.update(|state| {
            state.value.status = status.into();
            state.value.message = message.into();
            state.value.completed_rows += 1;
            state.value.estimated_remaining_seconds = None;
            state.estimate = None;
        })
    });
}
pub(super) fn media(document: &ProbeDocument) {
    let _ = CURRENT.try_with(|context| {
        context.update(|state| {
            if state.value.duration_seconds.is_none() {
                state.value.duration_seconds = duration(document);
            }
        })
    });
}
pub(super) fn grain_estimate(width: u32, height: u32, frames: u64) {
    let seconds = f64::from(width) * f64::from(height) * frames as f64 / 7_200_000.0;
    let _ = CURRENT.try_with(|context| context.update(|state| {
        state.estimate = Some(state.value.elapsed_seconds + seconds);
        state.value.message = format!("Approximate grain measurement cost: {width}×{height}, {frames} frames at the fork's 7.2 megapixels/s reference rate; hardware, decoding and denoising change actual time.");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn fragmented_observer_ignores_stdout_and_bounds_pending_bytes() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let sink = values.clone();
        with_utility_progress(move |value| sink.lock().unwrap().push(value), async {
            CURRENT.with(|context| {
                context.state.lock().unwrap().last_emit = Instant::now() - Duration::from_secs(1);
                context.observe(false, b"out_time_us=9000000\n");
                context.observe(true, &vec![b'x'; 4096]);
                context.observe(true, b"\nout_time_us=12");
                context.observe(true, b"50000\n");
                assert!(context.state.lock().unwrap().pending.len() <= 1024);
            });
        })
        .await;
        let values = values.lock().unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].progress_seconds, Some(1.25));
    }

    #[tokio::test]
    async fn paused_time_does_not_reduce_measurement_eta() {
        let control = crate::supervisor::PauseControl::default();
        let values = Arc::new(Mutex::new(Vec::new()));
        let sink = values.clone();
        crate::supervisor::with_pause(
            control.clone(),
            with_utility_progress(move |value| sink.lock().unwrap().push(value), async {
                grain_estimate(1920, 1080, 240);
                control.set_paused(true).unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
                CURRENT.with(|context| {
                    context.state.lock().unwrap().last_emit =
                        Instant::now() - Duration::from_secs(1)
                });
                phase("measure", "Measuring source grain");
                control.set_paused(false).unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
                CURRENT.with(|context| {
                    context.state.lock().unwrap().last_emit =
                        Instant::now() - Duration::from_secs(1)
                });
                phase("measure", "Measuring source grain");
            }),
        )
        .await;
        let values = values.lock().unwrap();
        assert!(values[1].elapsed_seconds - values[0].elapsed_seconds < 0.02);
        assert!(
            (values[1].estimated_remaining_seconds.unwrap()
                - values[0].estimated_remaining_seconds.unwrap())
            .abs()
                < 0.02
        );
        assert!(values[2].elapsed_seconds - values[1].elapsed_seconds >= 0.03);
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg for live progress delivery"]
    async fn real_ffmpeg_reports_position_before_capture_completes() {
        let path = crate::discovery::find_executable(&["ffmpeg"])
            .await
            .unwrap()
            .unwrap();
        let values = Arc::new(Mutex::new(Vec::new()));
        let sink = values.clone();
        let task = tokio::spawn(with_utility_progress(
            move |value| sink.lock().unwrap().push(value),
            async move {
                let (_owner, cancel) = watch::channel(false);
                run_capture(
                    &CommandSpec {
                        executable: path,
                        args: [
                            "-v",
                            "error",
                            "-progress",
                            "pipe:2",
                            "-stats_period",
                            "0.1",
                            "-re",
                            "-f",
                            "lavfi",
                            "-i",
                            "color=s=32x32:r=24:d=2",
                            "-threads:v",
                            "1",
                            "-pix_fmt",
                            "yuv420p",
                            "-f",
                            "rawvideo",
                            "-",
                        ]
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                        cwd: None,
                    },
                    cancel,
                    OUTPUT_LIMIT,
                    Duration::from_secs(10),
                )
                .await
                .unwrap()
            },
        ));
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(
            !task.is_finished(),
            "the bounded real source should still be running"
        );
        assert!(
            values
                .lock()
                .unwrap()
                .iter()
                .any(|value| value.progress_seconds.is_some_and(|seconds| seconds > 0.1)),
            "progress must reach the caller before completion"
        );
        let output = task.await.unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 48 * 32 * 32 * 3 / 2);
        assert!(output.stdout[..1024].iter().all(|byte| *byte == 16));
        assert!(output.stdout[1024..1536].iter().all(|byte| *byte == 128));
    }
}
