//! Source frame planning from packet timestamps, with the complete decoded-frame
//! validation running beside the encoder instead of before it.
//!
//! Every source frame must still pass the strict per-frame checks documented in
//! `docs/vulkan-libplacebo-provider.md` and `docs/source-dolby-combinations.md`:
//! constant-rate timestamps, progressive fields, dimensions and pixel format,
//! color declarations, static HDR metadata and a valid Dolby Vision RPU. Those
//! need decoded frames, and decoding is the dominant preparation cost: a 4K
//! HEVC source decodes at roughly 200 frames per second even with every core,
//! about 11 minutes for a feature, before the encoder could start.
//!
//! Planning the encode needs much less: the frame count, the cadence and the
//! first frame's declarations. Those come from every packet's presentation
//! time (demuxed, not decoded) checked with the same cadence validator, plus a
//! complete check of the first decoded frames. The unchanged decoded scan then
//! runs concurrently with the encoder. A rejected frame, or a decoded count,
//! cadence or HDR plan that differs from the packet plan, stops the encoder and
//! fails the job, and `finalize_verified` publishes nothing until the scan has
//! passed. Sources whose packets cannot stand in for decoded frames (missing
//! timestamps, discarded or corrupt packets, a first decoded frame that is not
//! the first packet time) keep the previous blocking scan.
use super::encode_plan::{Frame, Plan, Validation};
use super::*;
use std::sync::Arc;

tokio::task_local! { static VERIFICATION: Arc<Verification>; }

/// Packets decoded for the first-frame checks. References precede dependents
/// in decode order, so a prefix decodes completely; a few packets also cover
/// leading pictures a decoder legitimately drops.
const FIRST_PACKETS: u32 = 16;

pub(in crate::jobs) struct Verification {
    stop: watch::Sender<bool>,
    state: std::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    handle: Option<tokio::task::JoinHandle<()>>,
    started: bool,
    verified: bool,
    failure: Option<AppError>,
}

impl Verification {
    pub(in crate::jobs) fn new(parent: &watch::Receiver<bool>) -> Arc<Self> {
        let (stop, _) = watch::channel(*parent.borrow());
        Arc::new(Self {
            stop,
            state: std::sync::Mutex::new(State::default()),
        })
    }

    /// The job's cancellation, also raised when the background scan fails so
    /// the encoder stops as soon as a source frame is rejected.
    pub(in crate::jobs) fn cancel(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|cause| cause.into_inner())
    }

    fn fail(&self, error: AppError) {
        self.state().failure.get_or_insert(error);
        self.stop.send_replace(true);
    }

    /// Run one job with this verification scope. A background failure replaces
    /// the error its cancellation caused; an unfinished scan is stopped.
    pub(in crate::jobs) async fn run<T>(
        self: &Arc<Self>,
        parent: &watch::Receiver<bool>,
        future: impl std::future::Future<Output = Result<T, AppError>>,
    ) -> Result<T, AppError> {
        let mut parent = parent.clone();
        let stop = self.stop.clone();
        let forward = async move {
            loop {
                if *parent.borrow_and_update() {
                    stop.send_replace(true);
                    break;
                }
                if parent.changed().await.is_err() {
                    break;
                }
            }
            std::future::pending::<()>().await
        };
        let result = VERIFICATION
            .scope(self.clone(), async {
                tokio::select! {
                    biased;
                    result = future => result,
                    () = forward => unreachable!("cancel forwarding never completes"),
                }
            })
            .await;
        let mut state = self.state();
        if let Some(handle) = state.handle.take() {
            handle.abort();
        }
        match result {
            Err(error) => Err(state.failure.take().unwrap_or(error)),
            Ok(value) => Ok(value),
        }
    }
}

/// Whether a running job can verify its source beside the encoder.
pub(in crate::jobs) fn available() -> bool {
    VERIFICATION.try_with(|_| ()).is_ok()
}

/// Whether a background source scan was started and has not yet passed.
pub(in crate::jobs) fn pending() -> bool {
    VERIFICATION
        .try_with(|verification| {
            let state = verification.state();
            state.started && !state.verified
        })
        .unwrap_or(false)
}

/// What the complete decoded scan must reproduce.
pub(in crate::jobs) struct Expected {
    count: usize,
    cadence: (u32, u32, bool),
    tolerance: f64,
    hdr: Vec<OsString>,
}

impl Expected {
    pub(in crate::jobs) fn new(plan: &Plan, count: usize) -> Self {
        Self {
            count,
            cadence: (plan.fps_num, plan.fps_den, plan.cadence_reconciled),
            tolerance: plan.tolerance,
            hdr: plan.hdr_arguments(),
        }
    }

    fn check(&self, plan: &Plan, count: usize) -> Result<(), AppError> {
        let fail = |message: String| AppError::new("DECODE_VALIDATION_FAILED", message, None);
        if count != self.count {
            return Err(fail(format!(
                "The complete source decoded to {count} frames, but its packets list {} frames.",
                self.count
            )));
        }
        if (plan.fps_num, plan.fps_den, plan.cadence_reconciled) != self.cadence
            || plan.tolerance != self.tolerance
        {
            return Err(fail(
                "The decoded source frame clock differs from its packet timestamps.".into(),
            ));
        }
        if plan.hdr_arguments() != self.hdr {
            return Err(fail(
                "The decoded source HDR declarations differ from its first frames.".into(),
            ));
        }
        Ok(())
    }
}

/// Start the complete decoded-source scan beside the encoder. `plan` is the
/// plan before any frame was seen, exactly what the blocking scan received.
pub(in crate::jobs) fn spawn(
    manager: JobManager,
    id: String,
    ffprobe: PathBuf,
    input: PathBuf,
    plan: Plan,
    stream: metadata::Stream,
    expected: Expected,
) {
    let verification = VERIFICATION
        .try_with(Clone::clone)
        .expect("checked verification scope");
    let cancel = verification.cancel();
    let pause = crate::supervisor::scoped_pause();
    {
        let mut state = verification.state();
        if let Some(previous) = state.handle.take() {
            previous.abort();
        }
        state.started = true;
        state.verified = false;
    }
    let task = verification.clone();
    let handle = tokio::spawn(async move {
        let scan = async {
            let mut plan = plan;
            let count = super::encode::frame_scan(
                &ffprobe, &input, &mut plan, &stream, false, &cancel, None,
            )
            .await?;
            expected.check(&plan, count)?;
            Ok::<_, AppError>(count)
        };
        // Pausing the job also suspends this scan's FFprobe.
        let result = match pause {
            Some(control) => crate::supervisor::with_pause(control, scan).await,
            None => scan.await,
        };
        match result {
            Ok(count) => {
                task.state().verified = true;
                manager
                    .change(&id, |snapshot| {
                        append_log(
                            snapshot,
                            format!("Verified all {count} decoded source frames: timestamps, progressive scan, dimensions, color, HDR and Dolby Vision metadata."),
                        )
                    })
                    .await;
            }
            // A canceled job reports its own cancellation.
            Err(_) if *cancel.borrow() => {}
            Err(error) => {
                manager
                    .change(&id, |snapshot| {
                        append_log(
                            snapshot,
                            format!(
                                "Decoded source verification failed; stopping the encode: {}",
                                error.message
                            ),
                        )
                    })
                    .await;
                task.fail(error);
            }
        }
    });
    verification.state().handle = Some(handle);
}

/// Wait for a background source scan before publication.
pub(in crate::jobs) async fn settle(cancel: &watch::Receiver<bool>) -> Result<(), AppError> {
    let Ok(verification) = VERIFICATION.try_with(Clone::clone) else {
        return Ok(());
    };
    let handle = verification.state().handle.take();
    if let Some(handle) = handle {
        let joined = handle.await;
        if joined.is_err() {
            verification.fail(AppError::new(
                "DECODE_VALIDATION_FAILED",
                "The decoded source verification stopped unexpectedly.",
                None,
            ));
        }
    }
    check_cancel(cancel)?;
    let mut state = verification.state();
    if let Some(failure) = state.failure.take() {
        return Err(failure);
    }
    if state.started && !state.verified {
        return Err(AppError::new(
            "DECODE_VALIDATION_FAILED",
            "The decoded source verification did not complete.",
            None,
        ));
    }
    Ok(())
}

/// Why packets cannot stand in for decoded frames; the caller then decodes
/// the complete source before encoding, as before.
pub(in crate::jobs) type Fallback = String;

/// Plan the frame count and cadence from packet times and the first decoded
/// frames. A semantic rejection (for example variable timestamps) is an error
/// exactly as the decoded scan would report it.
pub(in crate::jobs) async fn plan_from_packets(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    stream: &metadata::Stream,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<(Plan, usize), Fallback>, AppError> {
    let times = match packet_times(ffprobe, input, stream.index, cancel).await? {
        Ok(times) => times,
        Err(reason) => return Ok(Err(reason)),
    };
    let first = match first_frame(ffprobe, input, plan, stream.index, cancel).await? {
        Ok(first) => first,
        Err(reason) => return Ok(Err(reason)),
    };
    let first_time = first
        .timestamp()
        .and_then(|value| value.parse::<f64>().ok());
    if !first_time.is_some_and(|time| (time - times[0]).abs() <= 0.000_001) {
        return Ok(Err(
            "the first decoded frame is not the first packet time".into()
        ));
    }
    let mut validation = Validation::timing(plan.clone(), stream.clone());
    validation.push(&first);
    for (position, time) in times[1..].iter().enumerate() {
        if position % 8192 == 0 {
            check_cancel(cancel)?;
        }
        validation.push(&Frame::timestamp_only(format!("{time:.6}")));
    }
    validation.finish().map(Ok)
}

/// Every packet's presentation time in presentation order, without decoding.
async fn packet_times(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<Vec<f64>, Fallback>, AppError> {
    check_cancel(cancel)?;
    let args = packet_args(input, index);
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(60 * 60),
        |reader| Ok(parse_packets(std::io::BufReader::new(reader))),
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(Err("the packet listing reported errors".into()));
    }
    Ok(output.value)
}

fn packet_args(input: &Path, index: u32) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(index.to_string().into());
    args.extend(
        [
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
            "-i",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args.push(input.as_os_str().to_owned());
    args
}

/// Read the complete listing even after a fallback reason: the supervisor
/// rejects a parser that leaves stdout unread.
fn parse_packets(reader: impl std::io::BufRead) -> Result<Vec<f64>, Fallback> {
    let mut times = Vec::new();
    let mut fallback = None;
    for (position, line) in reader.lines().enumerate() {
        let line = line.map_err(|cause| cause.to_string())?;
        let line = line.trim();
        if line.is_empty() || fallback.is_some() {
            continue;
        }
        let packet = (|| {
            let (time, flags) = line
                .split_once(',')
                .ok_or_else(|| format!("packet {position} has an unreadable listing"))?;
            let time = time
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("packet {position} has no presentation time"))?;
            if flags.contains('D') || flags.contains('C') {
                return Err(format!("packet {position} is discarded or corrupt"));
            }
            if times.is_empty() && !flags.contains('K') {
                return Err("the first packet is not a keyframe".to_owned());
            }
            Ok(time)
        })();
        match packet {
            Ok(time) => times.push(time),
            Err(reason) => fallback = Some(reason),
        }
    }
    if let Some(reason) = fallback {
        return Err(reason);
    }
    if times.is_empty() {
        return Err("the stream has no packets".into());
    }
    times.sort_by(f64::total_cmp);
    Ok(times)
}

/// The first decoded frame, with exactly the fields of the complete scan.
async fn first_frame(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Result<Frame, Fallback>, AppError> {
    check_cancel(cancel)?;
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut args = super::encode::source_scan_args(input, index, threads, plan);
    let show = args
        .iter()
        .position(|arg| arg == "-show_frames")
        .expect("frame scan arguments");
    args.splice(
        show..show,
        [
            "-read_intervals".into(),
            format!("%+#{FIRST_PACKETS}").into(),
        ],
    );
    let output = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(10 * 60),
        |reader| {
            let mut first = None;
            super::encode::frame_scan::parse(reader, |frame: Frame| {
                first.get_or_insert(frame);
            })?;
            Ok(first)
        },
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(Err(
            "the first frames could not be decoded without errors".into()
        ));
    }
    Ok(output
        .value
        .ok_or_else(|| "no frame decoded from the first packets".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_give_presentation_order_and_refuse_what_decoding_would_change() {
        let listing = "0.000000,K__\n0.083417,___\n0.041708,___\n\n0.125125,___\n";
        assert_eq!(
            parse_packets(listing.as_bytes()).unwrap(),
            [0.0, 0.041708, 0.083417, 0.125125]
        );
        for (listing, reason) in [
            ("", "no packets"),
            ("0.041708,___\n0.000000,K__\n", "keyframe"),
            ("0.000000,K__\nN/A,___\n", "presentation time"),
            ("-0.041708,KD_\n0.000000,K__\n", "discarded"),
            ("0.000000,K__\n0.041708,_C_\n", "corrupt"),
            ("0.000000\n", "unreadable"),
        ] {
            let error = parse_packets(listing.as_bytes()).unwrap_err();
            assert!(error.contains(reason), "{listing:?}: {error}");
        }
    }

    #[test]
    fn packet_listing_demuxes_one_stream_without_decoding() {
        let args = packet_args(Path::new("movie.mp4"), 2)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-select_streams",
                "2",
                "-show_entries",
                "packet=pts_time,flags",
                "-of",
                "csv=p=0",
                "-i",
                "movie.mp4"
            ]
        );
    }
}
