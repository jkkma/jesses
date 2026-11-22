//! Packet clocks and the first decoded frame say nothing about later HDR
//! side data. Read it alongside encoding and before publication.

use super::*;
use std::{
    future::Future,
    ops::Range,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

tokio::task_local! {
    static HDR_CHECKS: Arc<HdrChecks>;
}

pub(in crate::jobs) struct HdrChecks {
    cancel: watch::Sender<bool>,
    failure: Mutex<Option<AppError>>,
}

impl HdrChecks {
    pub(in crate::jobs) fn new(parent: &watch::Receiver<bool>) -> Arc<Self> {
        Arc::new(Self {
            cancel: watch::channel(*parent.borrow()).0,
            failure: Mutex::new(None),
        })
    }

    pub(in crate::jobs) fn cancel(&self) -> watch::Receiver<bool> {
        self.cancel.subscribe()
    }

    pub(super) fn reject(&self, error: AppError) {
        self.failure
            .lock()
            .expect("HDR failure lock")
            .get_or_insert(error);
        self.cancel.send_replace(true);
    }

    pub(in crate::jobs) async fn run<T>(
        self: &Arc<Self>,
        parent: &watch::Receiver<bool>,
        encode: impl std::future::Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        let mut parent = parent.clone();
        let forward_cancel = async {
            if parent.wait_for(|value| *value).await.is_ok() {
                self.cancel.send_replace(true);
            }
            std::future::pending::<()>().await
        };
        let result = HDR_CHECKS
            .scope(self.clone(), async {
                tokio::select! {
                    biased;
                    result = encode => result,
                    () = forward_cancel => unreachable!(),
                }
            })
            .await;
        match self.failure.lock().expect("HDR failure lock").take() {
            Some(error) => Err(error),
            None => result,
        }
    }
}

pub(in crate::jobs) struct HdrCheck(JoinHandle<Result<Option<usize>, AppError>>);

impl HdrCheck {
    pub(in crate::jobs) fn start(
        ffprobe: &Path,
        input: &Path,
        plan: &Plan,
        selection: Option<Range<usize>>,
        exact_source_frames: Option<usize>,
    ) -> Self {
        let ffprobe = ffprobe.to_owned();
        let input = input.to_owned();
        let plan = plan.clone();
        Self::spawn(move |checks, cancel| async move {
            inspect(
                &ffprobe,
                &input,
                plan,
                selection,
                exact_source_frames,
                checks,
                &cancel,
            )
            .await
            .map(Some)
        })
    }

    pub(in crate::jobs) fn start_packets(
        ffmpeg: &Path,
        ffprobe: &Path,
        input: &Path,
        plan: &Plan,
        source: PacketHdrSource,
        exact_source_frames: Option<usize>,
    ) -> Self {
        let ffmpeg = ffmpeg.to_owned();
        let ffprobe = ffprobe.to_owned();
        let input = input.to_owned();
        let plan = plan.clone();
        Self::spawn(move |checks, cancel| async move {
            if packet_hdr::inspect(
                &ffmpeg,
                &ffprobe,
                &input,
                plan.video_index,
                source,
                checks.clone(),
                &cancel,
            )
            .await?
            {
                return Ok(None);
            }
            inspect(
                &ffprobe,
                &input,
                plan,
                None,
                exact_source_frames,
                checks,
                &cancel,
            )
            .await
            .map(Some)
        })
    }

    fn spawn<F, Fut>(scan: F) -> Self
    where
        F: FnOnce(Arc<HdrChecks>, watch::Receiver<bool>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<usize>, AppError>> + Send + 'static,
    {
        let checks = HDR_CHECKS.with(Arc::clone);
        let pause = supervisor::scoped_pause();
        Self(tokio::spawn(async move {
            let cancel = checks.cancel();
            let scan = scan(checks.clone(), cancel.clone());
            let result = match pause {
                Some(control) => supervisor::with_pause(control, scan).await,
                None => scan.await,
            };
            if let Err(error) = &result
                && !*cancel.borrow()
            {
                checks.reject(error.clone());
            }
            result
        }))
    }

    pub(in crate::jobs) async fn finish(
        mut self,
        producer_frames: Option<usize>,
    ) -> Result<(), AppError> {
        let scanned = (&mut self.0).await.map_err(|error| {
            AppError::new(
                "DECODE_VALIDATION_FAILED",
                format!("Source HDR verification did not finish: {error}"),
                None,
            )
        })??;
        if let (Some(scanned), Some(expected)) = (scanned, producer_frames)
            && scanned != expected
        {
            return Err(count_mismatch(scanned, expected));
        }
        Ok(())
    }
}

impl Drop for HdrCheck {
    fn drop(&mut self) {
        // Early exit drops the supervisor-owned child and closes the parser pipe.
        self.0.abort();
    }
}

#[derive(serde::Deserialize)]
struct FrameSideData {
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
}

async fn inspect(
    ffprobe: &Path,
    input: &Path,
    plan: Plan,
    selection: Option<Range<usize>>,
    exact_source_frames: Option<usize>,
    checks: Arc<HdrChecks>,
    cancel: &watch::Receiver<bool>,
) -> Result<usize, AppError> {
    let mut args = super::super::encode::hdr_scan_args(input, plan.video_index);
    let selected_end = selection.as_ref().map(|range| range.end);
    if let (Some(end), Some(total)) = (selected_end, exact_source_frames)
        && end < total
    {
        // Packet planning established frame/packet correspondence for this trim.
        // Decode enough extra packets for delayed and reordered pictures.
        let position = args.iter().position(|arg| arg == "-i").expect("scan input");
        args.splice(
            position..position,
            [
                "-read_intervals".into(),
                format!("%+#{}", end.saturating_add(64)).into(),
            ],
        );
    }
    let path = input.to_string_lossy().into_owned();
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            let mut count = 0;
            let mut failed = false;
            super::super::encode::frame_scan::parse(reader, |frame: FrameSideData| {
                let frame_index = count;
                count += 1;
                if failed
                    || selection
                        .as_ref()
                        .is_some_and(|range| !range.contains(&frame_index))
                {
                    return;
                }
                if let Err(mut error) = plan.validate_frame_side_data(&frame.side_data_list, false)
                {
                    error.path = Some(path.clone());
                    checks.reject(error);
                    failed = true;
                }
            })?;
            Ok(count)
        },
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "DECODE_VALIDATION_FAILED",
            format!(
                "Could not verify the source frames for HDR side data: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ),
            input,
        ));
    }
    if result.value < selected_end.unwrap_or(1) {
        return Err(count_mismatch(result.value, selected_end.unwrap_or(1)));
    }
    if selected_end.is_none()
        && let Some(expected) = exact_source_frames
        && result.value != expected
    {
        return Err(count_mismatch(result.value, expected));
    }
    Ok(result.value)
}

fn count_mismatch(scanned: usize, expected: usize) -> AppError {
    AppError::new(
        "DECODE_VALIDATION_FAILED",
        format!("Source HDR verification inspected {scanned} frames but expected {expected}."),
        None,
    )
}
