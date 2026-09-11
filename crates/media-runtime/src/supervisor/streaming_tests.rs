use super::*;
use std::{
    io::{BufRead, Read},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::ReadBuf;

const LARGE_CHUNKS: usize = 1280;
const LARGE_BYTES: u64 = (LARGE_CHUNKS * PIPE_BYTES) as u64;

pub(super) fn large_output() {
    let diagnostics = std::thread::spawn(|| {
        let mut stderr = std::io::stderr().lock();
        for _ in 0..800 {
            stderr.write_all(&[0xff; RECORD_BYTES]).unwrap();
        }
        stderr.write_all(b"newest diagnostics").unwrap();
        stderr.flush().unwrap();
    });
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(b"\nSTREAM_PAYLOAD_BEGIN\n").unwrap();
    let block: Vec<_> = (0..PIPE_BYTES).map(|index| (index % 256) as u8).collect();
    for _ in 0..LARGE_CHUNKS {
        stdout.write_all(&block).unwrap();
    }
    stdout.flush().unwrap();
    diagnostics.join().unwrap();
    std::process::exit(0);
}

fn count_bytes(reader: &mut dyn Read) -> Result<u64, String> {
    std::io::copy(reader, &mut std::io::sink()).map_err(|error| error.to_string())
}

struct CountedReader(Arc<AtomicU64>);
impl AsyncRead for CountedReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let count = buffer.remaining();
        buffer.initialize_unfilled().fill(0xff);
        buffer.advance(count);
        self.0.fetch_add(count as u64, Ordering::SeqCst);
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn blocked_streaming_parser_applies_fixed_memory_backpressure() {
    let count = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::channel(2);
    let task = tokio::spawn(forward_stdout(CountedReader(Arc::clone(&count)), sender));
    tokio::time::timeout(Duration::from_secs(2), async {
        while count.load(Ordering::SeqCst) < (PIPE_BYTES * 3) as u64 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    // Two queued chunks plus one waiting to be sent. No further stdout bytes
    // are read until the parser consumes something or closes its reader.
    assert_eq!(count.load(Ordering::SeqCst), (PIPE_BYTES * 3) as u64);
    drop(receiver);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn stdout_larger_than_64_mib_is_lossless_with_bounded_stderr() {
    let fixture = Fixture::new("stream-large");
    let (_cancel, receiver) = watch::channel(false);
    let output = run_streaming_stdout(
        &fixture.spec(),
        receiver,
        18,
        Duration::from_secs(30),
        |reader| {
            let mut reader = std::io::BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                if reader
                    .read_line(&mut line)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    return Err("Missing binary payload marker".into());
                }
                if line == "STREAM_PAYLOAD_BEGIN\n" {
                    break;
                }
            }
            let mut bytes = vec![0; PIPE_BYTES];
            let mut total = 0_u64;
            loop {
                let count = reader.read(&mut bytes).map_err(|error| error.to_string())?;
                if count == 0 {
                    return Ok(total);
                }
                if bytes[..count]
                    .iter()
                    .enumerate()
                    .any(|(index, &byte)| byte != ((total + index as u64) % 256) as u8)
                {
                    return Err("Binary payload was altered or dropped".into());
                }
                total += count as u64;
            }
        },
    )
    .await
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.value, LARGE_BYTES);
    assert!(output.value > 64 * 1024 * 1024);
    assert_eq!(output.stderr, b"newest diagnostics");
    assert_dead(&[fixture.pid(0).await]).await;
}

#[tokio::test]
async fn streaming_stderr_retains_exact_tail_including_zero_limit() {
    let bytes: Vec<_> = (0..RECORD_BYTES * 5 + 71)
        .map(|index| (index % 251) as u8)
        .collect();
    for limit in [0, 1, 513, RECORD_BYTES, RECORD_BYTES + 17, bytes.len() + 1] {
        let output = capture_tail(bytes.as_slice(), limit).await.unwrap();
        assert_eq!(output, bytes[bytes.len().saturating_sub(limit)..]);
    }
}

#[tokio::test]
async fn streaming_retains_exit_failure_after_stdout_eof() {
    let fixture = Fixture::new("nonzero");
    let (_cancel, receiver) = watch::channel(false);
    let output = run_streaming_stdout(
        &fixture.spec(),
        receiver,
        1024,
        Duration::from_secs(10),
        count_bytes,
    )
    .await
    .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(output.value > 0); // The native test harness writes its header.
}

#[tokio::test]
async fn streaming_parser_rejection_stops_and_awaits_grandchildren() {
    let fixture = Fixture::new("stream-reject-tree");
    let (_cancel, receiver) = watch::channel(false);
    let result = run_streaming_stdout(
        &fixture.spec(),
        receiver,
        1024,
        Duration::from_secs(10),
        |reader| {
            let mut reader = std::io::BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                if reader
                    .read_line(&mut line)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    return Err::<(), _>("Missing tree marker".into());
                }
                if line == "STREAM_TREE_READY\n" {
                    return Err("Rejected malformed output".into());
                }
            }
        },
    )
    .await;
    assert!(
        matches!(result, Err(SupervisorError::OutputParse(error)) if error == "Rejected malformed output")
    );
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    assert!(pids.iter().all(|&pid| !alive(pid)));
}

#[tokio::test]
async fn streaming_parser_cannot_succeed_with_unread_stdout() {
    let fixture = Fixture::new("small");
    let (_cancel, receiver) = watch::channel(false);
    let result = run_streaming_stdout(
        &fixture.spec(),
        receiver,
        1024,
        Duration::from_secs(10),
        |_| Ok(42),
    )
    .await;
    assert!(
        matches!(result, Err(SupervisorError::OutputParse(error)) if error.contains("consuming all stdout"))
    );
}

#[tokio::test]
async fn streaming_cancellation_stops_tree_and_joins_parser() {
    let fixture = Fixture::new("tree");
    let spec = fixture.spec();
    let (cancel, receiver) = watch::channel(false);
    let parser_done = Arc::new(AtomicBool::new(false));
    let parser_marker = Arc::clone(&parser_done);
    let task = tokio::spawn(async move {
        run_streaming_stdout(
            &spec,
            receiver,
            1024,
            Duration::from_secs(20),
            move |reader| {
                let result = count_bytes(reader);
                parser_marker.store(true, Ordering::Release);
                result
            },
        )
        .await
    });
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    cancel.send(true).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(SupervisorError::Cancelled)
    ));
    assert!(parser_done.load(Ordering::Acquire));
    assert!(pids.iter().all(|&pid| !alive(pid)));
}

#[tokio::test]
async fn streaming_timeout_stops_tree_and_joins_parser() {
    let fixture = Fixture::new("tree");
    let spec = fixture.spec();
    let (_cancel, receiver) = watch::channel(false);
    let parser_done = Arc::new(AtomicBool::new(false));
    let parser_marker = Arc::clone(&parser_done);
    let task = tokio::spawn(async move {
        run_streaming_stdout(
            &spec,
            receiver,
            1024,
            Duration::from_secs(2),
            move |reader| {
                let result = count_bytes(reader);
                parser_marker.store(true, Ordering::Release);
                result
            },
        )
        .await
    });
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    assert!(matches!(task.await.unwrap(), Err(SupervisorError::Timeout)));
    assert!(parser_done.load(Ordering::Acquire));
    assert!(pids.iter().all(|&pid| !alive(pid)));
}

#[tokio::test]
async fn aborting_streaming_future_stops_tree_and_releases_parser() {
    let fixture = Fixture::new("tree");
    let spec = fixture.spec();
    let (_cancel, receiver) = watch::channel(false);
    let parser_done = Arc::new(AtomicBool::new(false));
    let parser_marker = Arc::clone(&parser_done);
    let task = tokio::spawn(async move {
        run_streaming_stdout(
            &spec,
            receiver,
            1024,
            Duration::from_secs(20),
            move |reader| {
                let result = count_bytes(reader);
                parser_marker.store(true, Ordering::Release);
                result
            },
        )
        .await
    });
    let pids = [
        fixture.pid(0).await,
        fixture.pid(1).await,
        fixture.pid(2).await,
    ];
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_dead(&pids).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !parser_done.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the parser worker must be released when its caller is aborted");
}

#[tokio::test]
async fn streaming_cancellation_before_launch_does_not_start_tool_or_parser() {
    let fixture = Fixture::new("sleep");
    let (_cancel, receiver) = watch::channel(true);
    let result = run_streaming_stdout::<(), _>(
        &fixture.spec(),
        receiver,
        1024,
        Duration::from_secs(10),
        |_| panic!("a cancelled run must never start its parser"),
    )
    .await;
    assert!(matches!(result, Err(SupervisorError::Cancelled)));
    assert!(!fixture.0.join("pid").exists());
}
