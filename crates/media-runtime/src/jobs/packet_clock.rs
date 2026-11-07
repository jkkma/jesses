//! The presentation clock of a copied video stream, read from its packet
//! timestamps without decoding.
//!
//! A decoder outputs one frame per packet of the codecs written here (H.264
//! and HEVC access units, VP9 superframes, AV1 temporal units), at that
//! packet's presentation time and in presentation order. So for a stream
//! whose packets and decoder configuration have been proven byte-identical
//! to a stream that was decoded completely and verified frame by frame,
//! sorting the packet times gives the decoded frame clock. This module does
//! not establish that precondition; callers must prove it first.
use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    path::Path,
    time::Duration,
};

use media_core::AppError;
use tokio::sync::watch;

use super::{check_cancel, files, mux, process_error};
use crate::supervisor;

/// The deepest packet reordering accepted. H.264 and HEVC reorder by at most
/// their 16-picture decoded picture buffer; AV1 and VP9 do not reorder at all.
/// A stream that reorders further fails rather than being trusted.
const REORDER_WINDOW: usize = 64;

/// The clock the copied frames must follow: frame `n` is presented at
/// `origin + n * frame_seconds`, within `tolerance`.
#[derive(Clone, Copy, Debug)]
pub(in crate::jobs) struct Expected {
    pub origin: f64,
    pub frame_seconds: f64,
    pub tolerance: f64,
    pub frames: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct Time(f64);

impl Eq for Time {}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Checks packet presentation times in decode order with constant memory.
/// After the first failure it records nothing more, but it still consumes
/// the whole packet list so that a read error takes precedence.
pub(in crate::jobs) struct Check {
    expected: Expected,
    pending: BinaryHeap<Reverse<Time>>,
    emitted: usize,
    previous: Option<f64>,
    error: Option<String>,
}

impl Check {
    pub(in crate::jobs) fn new(expected: Expected) -> Self {
        Self {
            expected,
            pending: BinaryHeap::with_capacity(REORDER_WINDOW + 1),
            emitted: 0,
            previous: None,
            error: None,
        }
    }

    pub(in crate::jobs) fn push(&mut self, pts: Option<f64>) {
        if self.error.is_some() {
            return;
        }
        let Some(pts) = pts.filter(|value| value.is_finite()) else {
            self.error = Some("A copied video packet has no usable presentation time.".into());
            return;
        };
        self.pending.push(Reverse(Time(pts)));
        if self.pending.len() > REORDER_WINDOW {
            self.emit();
        }
    }

    /// Takes the earliest pending time as the next presented frame.
    fn emit(&mut self) {
        let Some(Reverse(Time(time))) = self.pending.pop() else {
            return;
        };
        if self.error.is_some() {
            return;
        }
        let expected = self.expected.origin + self.emitted as f64 * self.expected.frame_seconds;
        if self.previous.is_some_and(|previous| time <= previous) {
            self.error = Some(
                "Copied video packet presentation times repeat, or reorder further than a decoder's picture buffer."
                    .into(),
            );
        } else if (time - expected).abs() > self.expected.tolerance {
            self.error = Some(format!(
                "Copied video frame {} is presented at {time:.6}s instead of {expected:.6}s on the verified constant frame clock.",
                self.emitted
            ));
        }
        self.previous = Some(time);
        self.emitted = self.emitted.saturating_add(1);
    }

    pub(in crate::jobs) fn finish(mut self) -> Result<usize, String> {
        while !self.pending.is_empty() {
            self.emit();
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.emitted != self.expected.frames {
            return Err(format!(
                "The copied video has {} packets; the verified encode has {} frames.",
                self.emitted, self.expected.frames
            ));
        }
        Ok(self.emitted)
    }
}

/// Reads every packet of one video stream and checks its presentation clock.
pub(in crate::jobs) async fn verify(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    expected: Expected,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    check_cancel(cancel)?;
    let output = supervisor::run_streaming_stdout(
        &mux::packet_spec(ffprobe, input, index),
        cancel.clone(),
        8192,
        Duration::from_secs(24 * 60 * 60),
        move |reader| {
            let mut check = Check::new(expected);
            mux::packets(reader, |packet| {
                check.push(packet.pts);
                Ok(())
            })?;
            Ok(check.finish())
        },
    )
    .await
    .map_err(|error| process_error(error, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(600)
            .collect();
        return Err(files::error(
            "OUTPUT_VALIDATION_FAILED",
            format!("FFprobe could not read every copied video packet: {detail}"),
            input,
        ));
    }
    output
        .value
        .map(|_| ())
        .map_err(|detail| files::error("OUTPUT_VALIDATION_FAILED", detail, input))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(frames: usize) -> Expected {
        Expected {
            origin: 0.5,
            frame_seconds: 1001.0 / 24000.0,
            tolerance: 0.001002,
            frames,
        }
    }

    fn time(frame: usize) -> Option<f64> {
        // Millisecond Matroska quantization of a 24000/1001 clock.
        Some(((0.5 + frame as f64 * 1001.0 / 24000.0) * 1000.0).round() / 1000.0)
    }

    fn run(
        expected: Expected,
        decode_order: impl IntoIterator<Item = Option<f64>>,
    ) -> Result<usize, String> {
        let mut check = Check::new(expected);
        for pts in decode_order {
            check.push(pts);
        }
        check.finish()
    }

    #[test]
    fn accepts_a_quantized_clock_in_b_frame_decode_order() {
        // I P B B pattern: each P is decoded before the two B-frames it follows.
        let mut order = vec![0];
        for group in (1..97).step_by(3) {
            order.extend([group + 2, group, group + 1]);
        }
        assert_eq!(run(expected(97), order.into_iter().map(time)), Ok(97));
    }

    #[test]
    fn rejects_a_frame_off_the_clock() {
        let mut times = (0..48).map(time).collect::<Vec<_>>();
        times[20] = times[20].map(|value| value + 0.004);
        let error = run(expected(48), times).unwrap_err();
        assert!(error.contains("frame 20"), "{error}");
    }

    #[test]
    fn rejects_a_dropped_or_extra_frame() {
        assert!(run(expected(48), (0..47).map(time)).is_err());
        assert!(run(expected(48), (0..49).map(time)).is_err());
        // A gap shifts every later frame off the clock.
        assert!(run(expected(47), (0..48).filter(|frame| *frame != 10).map(time)).is_err());
    }

    #[test]
    fn rejects_repeated_missing_or_unbounded_reordered_times() {
        let mut repeated = (0..48).map(time).collect::<Vec<_>>();
        repeated[30] = repeated[29];
        assert!(run(expected(48), repeated).is_err());

        let mut missing = (0..48).map(time).collect::<Vec<_>>();
        missing[5] = None;
        assert!(run(expected(48), missing).is_err());

        let frames = REORDER_WINDOW + 10;
        let late = (1..frames).map(time).chain([time(0)]);
        assert!(run(expected(frames), late).is_err());
    }
}
