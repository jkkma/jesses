//! Inline observations from the actual FFmpeg producer. User filters run once.
//! Each process/output stream owns a distinct identity-guarded metadata file.
use media_core::AppError;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::watch;

static NEXT: AtomicU64 = AtomicU64::new(1);
tokio::task_local! { static CURRENT: Observation; }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::jobs) struct ProducerClock {
    pub stream_index: u32,
    pub frames: u64,
    /// Encoder-pre timebase. Raw Y4M child encoders use their explicit frame
    /// clock rather than these upstream filter PTS values.
    pub time_base: (u32, u32),
    pub first_pts: i64,
    pub last_pts: i64,
    pub monotonic: bool,
    /// SHA256(previous digest || relative producer PTS i64 LE), zero seed.
    pub relative_pts_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::jobs) struct OutputContract {
    pub width: u32,
    pub height: u32,
    pub sar: String,
    pub numerator: u32,
    pub denominator: u32,
    pub frames: u64,
}
struct StatsFile {
    temporary: super::files::Temporary,
    tag: String,
    index: u32,
}
struct Invocation {
    files: Vec<StatsFile>,
}
struct State {
    output: PathBuf,
    invocations: Vec<Invocation>,
    error: Option<AppError>,
}
#[derive(Clone)]
pub(in crate::jobs) struct Observation(Arc<Mutex<State>>);

impl Observation {
    pub(in crate::jobs) fn new(output: &Path) -> Self {
        let absolute = if output.is_absolute() {
            Ok(output.to_owned())
        } else {
            std::env::current_dir().map(|directory| directory.join(output))
        };
        let (output, error) = match absolute {
            Ok(path) => (path, None),
            Err(cause) => (output.to_owned(), Some(invalid(&cause.to_string()))),
        };
        Self(Arc::new(Mutex::new(State {
            output,
            invocations: Vec::new(),
            error,
        })))
    }
    pub(in crate::jobs) fn arguments_for_streams(
        &self,
        indices: &[u32],
    ) -> Result<Vec<OsString>, AppError> {
        let mut state = self.0.lock().unwrap_or_else(|cause| cause.into_inner());
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if indices.is_empty() {
            return Err(invalid(
                "No video stream was selected for producer observations.",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut files = Vec::new();
        let mut args = Vec::new();
        let invocation = NEXT.fetch_add(1, Ordering::Relaxed);
        for &index in indices {
            if !seen.insert(index) {
                return Err(invalid("A producer video stream was instrumented twice."));
            }
            let id = format!("filter-stats-{}-{invocation}-{index}", std::process::id());
            let temporary = super::files::Temporary::create_extension(&state.output, &id, "txt")?;
            let tag = format!(
                "JESSES_SOURCE_FILTER_{}_{invocation}_{index}",
                std::process::id()
            );
            args.extend([
                format!("-stats_enc_pre:{index}").into(),
                temporary.path.as_os_str().to_owned(),
                format!("-stats_enc_pre_fmt:{index}").into(),
                format!("{tag} {{sidx}} {{tb}} {{pts}} {{n}}").into(),
            ]);
            files.push(StatsFile {
                temporary,
                tag,
                index,
            });
        }
        state.invocations.push(Invocation { files });
        Ok(args)
    }
    fn single_arguments(&self) -> Vec<OsString> {
        match self.arguments_for_streams(&[0]) {
            Ok(args) => args,
            Err(error) => {
                self.0
                    .lock()
                    .unwrap_or_else(|cause| cause.into_inner())
                    .error = Some(error);
                Vec::new()
            }
        }
    }
    /// The owned process has exited. Read bounded records, checking cancellation
    /// and the still-owned file identity before and after the complete scan.
    pub(in crate::jobs) async fn completed(
        &self,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Vec<ProducerClock>, AppError> {
        let state = self.0.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move||{
            let mut state=state.lock().unwrap_or_else(|cause|cause.into_inner());
            if let Some(error)=&state.error{return Err(error.clone());}
            for invocation in state.invocations.iter_mut().rev(){
                let mut active=false;
                for file in &invocation.files{if file.temporary.clone_file()?.metadata().map_err(|cause|invalid(&cause.to_string()))?.len()>0{active=true;}}
                if !active{continue;}
                let mut clocks=Vec::with_capacity(invocation.files.len());
                for file in &mut invocation.files{clocks.push(read_file(file,&cancel)?);}
                return Ok(clocks);
            }
            Err(invalid("The actual filtered producer did not supply frame observations. Its FFmpeg must support encoder-pre statistics."))
        }).await.map_err(|cause|invalid(&cause.to_string()))?
    }
}
fn invalid(message: &str) -> AppError {
    AppError::new("SOURCE_FILTER_OBSERVATION_INVALID", message, None)
}
fn read_file(
    file: &mut StatsFile,
    cancel: &watch::Receiver<bool>,
) -> Result<ProducerClock, AppError> {
    let mut input = file.temporary.clone_file()?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|cause| invalid(&cause.to_string()))?;
    let mut reader = BufReader::new(input);
    let mut clocks = BTreeMap::new();
    let mut line = Vec::with_capacity(256);
    loop {
        if *cancel.borrow() {
            return Err(AppError::new(
                "JOB_CANCELED",
                "The producer observation scan was canceled.",
                None,
            ));
        }
        line.clear();
        let mut limited = std::io::Read::take(&mut reader, 8193);
        let count = limited
            .read_until(b'\n', &mut line)
            .map_err(|cause| invalid(&cause.to_string()))?;
        if count == 0 {
            break;
        }
        if line.len() > 8192 || line.last() != Some(&b'\n') {
            return Err(invalid(
                "A private producer observation is oversized or truncated.",
            ));
        }
        let text = std::str::from_utf8(&line[..line.len() - 1])
            .map_err(|_| invalid("A private producer observation is not UTF-8."))?;
        let (tag, values) = text
            .split_once(' ')
            .ok_or_else(|| invalid("A private producer observation is malformed."))?;
        if tag != file.tag {
            return Err(invalid(
                "A private producer observation has the wrong invocation identity.",
            ));
        }
        record(values, &mut clocks).map_err(|cause| invalid(&cause))?;
    }
    file.temporary.clone_file()?;
    if clocks.len() != 1 || !clocks.contains_key(&file.index) {
        return Err(invalid(
            "A private producer file omitted or mixed selected output streams.",
        ));
    }
    Ok(clocks.remove(&file.index).unwrap())
}
fn record(values: &str, clocks: &mut BTreeMap<u32, ProducerClock>) -> Result<(), String> {
    let mut parts = values.split_whitespace();
    let stream = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("Invalid producer stream index")?;
    let (n, d) = parts
        .next()
        .and_then(|value| value.split_once('/'))
        .ok_or("Invalid producer timebase")?;
    let time_base = (
        n.parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or("Invalid producer timebase numerator")?,
        d.parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or("Invalid producer timebase denominator")?,
    );
    let pts = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or("Invalid producer PTS")?;
    let index = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("Invalid producer frame index")?;
    if parts.next().is_some() {
        return Err("Unexpected producer observation fields".into());
    }
    let clock = clocks.entry(stream).or_insert(ProducerClock {
        stream_index: stream,
        frames: 0,
        time_base,
        first_pts: pts,
        last_pts: pts,
        monotonic: true,
        relative_pts_sha256: [0; 32],
    });
    if clock.time_base != time_base || clock.frames != index {
        return Err("Producer frame observations changed timebase or skipped frames".into());
    }
    if clock.frames > 0 && pts <= clock.last_pts {
        clock.monotonic = false;
    }
    let relative = pts
        .checked_sub(clock.first_pts)
        .ok_or("Producer relative PTS overflow")?;
    let mut hash = Sha256::new();
    hash.update(clock.relative_pts_sha256);
    hash.update(relative.to_le_bytes());
    clock.relative_pts_sha256 = hash.finalize().into();
    clock.last_pts = pts;
    clock.frames = clock
        .frames
        .checked_add(1)
        .ok_or("Producer frame count overflow")?;
    Ok(())
}
pub(in crate::jobs) fn current() -> Option<Observation> {
    CURRENT.try_with(Clone::clone).ok()
}
pub(in crate::jobs) fn arguments() -> Vec<OsString> {
    current().map_or_else(Vec::new, |observation| observation.single_arguments())
}
pub(in crate::jobs) async fn with_observer<T>(
    observation: Observation,
    future: impl std::future::Future<Output = T>,
) -> T {
    CURRENT.scope(observation, future).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor;
    #[test]
    fn inline_records_are_bounded_per_stream_and_do_not_assume_filter_pts_are_encoder_clock() {
        let mut clocks = BTreeMap::new();
        for text in ["0 1/24 -2 0", "0 1/24 10 1", "1 1/1000 0 0", "0 1/24 11 2"] {
            record(text, &mut clocks).unwrap();
        }
        assert_eq!(clocks.len(), 2);
        assert_eq!(clocks[&0].frames, 3);
        for bad in [
            "0 1/24 12 4",
            "0 1/25 12 3",
            "0 0/24 12 3",
            "0 1/24 12 3 extra",
        ] {
            assert!(record(bad, &mut clocks).is_err());
        }
    }
    #[test]
    fn relative_clock_hash_detects_interior_changes() {
        let mut a = BTreeMap::new();
        let mut b = BTreeMap::new();
        for (index, pts) in [0, 1, 3, 4].into_iter().enumerate() {
            record(&format!("0 1/24 {pts} {index}"), &mut a).unwrap();
        }
        for (index, pts) in [0, 2, 3, 4].into_iter().enumerate() {
            record(&format!("0 1/24 {pts} {index}"), &mut b).unwrap();
        }
        assert_eq!(
            (a[&0].frames, a[&0].first_pts, a[&0].last_pts),
            (b[&0].frames, b[&0].first_pts, b[&0].last_pts)
        );
        assert_ne!(a[&0].relative_pts_sha256, b[&0].relative_pts_sha256);
    }
    #[tokio::test]
    async fn guarded_files_reject_truncated_records_and_stream_mixing_without_partial_acceptance() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-filter-counter")
            .join(format!(
                "files-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        let observation = Observation::new(&root.join("output.mkv"));
        observation.arguments_for_streams(&[0, 2]).unwrap();
        let files = {
            let state = observation.0.lock().unwrap();
            state.invocations[0]
                .files
                .iter()
                .map(|file| (file.temporary.path.clone(), file.tag.clone(), file.index))
                .collect::<Vec<_>>()
        };
        for (path, tag, index) in &files {
            std::fs::write(
                path,
                format!("{tag} {index} 1/24 0 0\n{tag} {index} 1/24 1 1\n"),
            )
            .unwrap();
        }
        let (_owner, cancel) = watch::channel(false);
        let clocks = observation.completed(&cancel).await.unwrap();
        assert_eq!(
            clocks
                .iter()
                .map(|clock| (clock.stream_index, clock.frames))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 2)]
        );
        let (path, tag, index) = &files[1];
        std::fs::write(path, format!("{tag} {index} 1/24 0 0\n{tag}")).unwrap();
        assert!(
            observation
                .completed(&cancel)
                .await
                .unwrap_err()
                .message
                .contains("truncated")
        );
        std::fs::write(path, format!("{tag} 0 1/24 0 0\n")).unwrap();
        assert!(
            observation
                .completed(&cancel)
                .await
                .unwrap_err()
                .message
                .contains("mixed")
        );
        drop(observation);
        assert!(files.iter().all(|(path, _, _)| !path.exists()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    #[ignore = "requires FFmpeg with encoder-pre statistics"]
    async fn actual_labeled_side_effect_filter_runs_once_and_private_records_never_reach_logs() {
        use std::time::Duration;
        use tokio::sync::mpsc;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-filter-counter")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        let effects = root.join("effects.txt");
        let log = root.join("process.log");
        let observation = Observation::new(&root.join("output.mkv"));
        let graph = format!(
            "[in]scale=80:48,metadata=mode=add:key=jesses:value=once,metadata=mode=print:file='{}'[out]",
            effects.display()
        );
        let mut args = [
            "-v",
            "warning",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=4:duration=1",
            "-vf",
            &graph,
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        args.extend(observation.arguments_for_streams(&[0]).unwrap());
        args.extend(["-f", "null", "-"].into_iter().map(OsString::from));
        let (_owner, cancel) = watch::channel(false);
        let (events, _receiver) = mpsc::channel(256);
        let result = supervisor::run(
            &supervisor::CommandSpec {
                executable: std::env::var_os("JESSES_FFMPEG")
                    .unwrap_or_else(|| "ffmpeg".into())
                    .into(),
                args,
                cwd: None,
            },
            cancel.clone(),
            events,
            &log,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            std::fs::read_to_string(&log).unwrap()
        );
        let clocks = observation.completed(&cancel).await.unwrap();
        assert_eq!(clocks.len(), 1);
        assert_eq!(clocks[0].frames, 4);
        let text = std::fs::read_to_string(effects).unwrap();
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("frame:"))
                .count(),
            4
        );
        assert_eq!(
            text.lines().filter(|line| *line == "jesses=once").count(),
            4
        );
        assert!(
            !std::fs::read_to_string(log)
                .unwrap()
                .contains("JESSES_SOURCE_FILTER_")
        );
        drop(observation);
        std::fs::remove_dir_all(root).unwrap();
    }
}
