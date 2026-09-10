use super::*;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::ReadBuf;

pub(super) const RAW_MARKER: &[u8] = b"RAW_BYTES_MUST_NEVER_ENTER_TEXT_LOGS\0\xff\xfe\r\n";
pub(super) const BINARY_CHUNKS: usize = 256;

pub(super) fn binary_block() -> [u8; RECORD_BYTES] {
    std::array::from_fn(|index| (index % 256) as u8)
}

fn fixtures(producer: &str, consumer: &str) -> (Fixture, Fixture) {
    let producer = Fixture::new(producer);
    let consumer = Fixture::new(consumer);
    std::fs::write(producer.0.join("peer"), consumer.0.to_str().unwrap()).unwrap();
    std::fs::write(consumer.0.join("peer"), producer.0.to_str().unwrap()).unwrap();
    (producer, consumer)
}

#[tokio::test]
async fn binary_pipeline_preserves_all_bytes_and_keeps_them_out_of_logs() {
    let (producer, consumer) = fixtures("binary-producer", "binary-consumer");
    let (_cancel, receiver) = watch::channel(false);
    let (events, _unread) = mpsc::channel(1);
    let log = producer.0.join("pipeline.log");
    let result = run_pipeline(
        &producer.spec(),
        &consumer.spec(),
        receiver,
        events,
        &log,
        Duration::from_secs(15),
    )
    .await
    .unwrap();
    assert!(result.producer_status.success());
    assert!(result.consumer_status.success());
    let output = std::fs::read(consumer.0.join("received.bin")).unwrap();
    assert_eq!(result.bytes_transferred, output.len() as u64);
    // The native test harness emits a small header; everything after the marker
    // is binary fixture data, including every possible byte and CR/LF/NUL.
    let marker = output
        .windows(RAW_MARKER.len())
        .position(|value| value == RAW_MARKER)
        .unwrap();
    let binary = &output[marker + RAW_MARKER.len()..];
    assert_eq!(binary.len(), BINARY_CHUNKS * RECORD_BYTES);
    assert!(
        binary
            .as_chunks::<RECORD_BYTES>()
            .0
            .iter()
            .all(|block| *block == binary_block())
    );
    let log = std::fs::read(log).unwrap();
    assert!(
        !log.windows(RAW_MARKER.len())
            .any(|value| value == RAW_MARKER)
    );
    let text = std::str::from_utf8(&log).unwrap();
    assert!(text.contains("[producer] producer diagnostic"));
    assert!(text.contains("[consumer] consumer diagnostic"));
    assert!(text.contains("[consumer] consumer complete"));
    assert_dead(&[producer.pid(0).await, consumer.pid(0).await]).await;
}

#[tokio::test]
async fn upstream_failure_cannot_be_masked_by_consumer_or_hang_at_eof() {
    let (producer, consumer) = fixtures("binary-fail", "read-then-hang");
    let (_cancel, receiver) = watch::channel(false);
    let (events, _unread) = mpsc::channel(1);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_pipeline(
            &producer.spec(),
            &consumer.spec(),
            receiver,
            events,
            &producer.0.join("pipeline.log"),
            Duration::from_secs(30),
        ),
    )
    .await
    .unwrap();
    assert!(
        matches!(result, Err(SupervisorError::StageFailed { stage: PipelineStage::Producer, status }) if status.code() == Some(7))
    );
    assert_dead(&[producer.pid(0).await, consumer.pid(0).await]).await;
}

#[tokio::test]
async fn consumer_early_success_and_failure_stop_the_producer() {
    for mode in ["early-consumer", "failed-consumer"] {
        let (producer, consumer) = fixtures("sleep", mode);
        let (_cancel, receiver) = watch::channel(false);
        let (events, _unread) = mpsc::channel(1);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run_pipeline(
                &producer.spec(),
                &consumer.spec(),
                receiver,
                events,
                &producer.0.join("pipeline.log"),
                Duration::from_secs(30),
            ),
        )
        .await
        .unwrap();
        if mode == "early-consumer" {
            assert!(
                matches!(result, Err(SupervisorError::EarlyConsumerExit)),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(SupervisorError::StageFailed { stage: PipelineStage::Consumer, status }) if status.code() == Some(9)),
                "{result:?}"
            );
        }
        assert_dead(&[producer.pid(0).await, consumer.pid(0).await]).await;
    }
}

#[tokio::test]
async fn cancel_or_drop_blocked_pipeline_stops_both_grandchild_trees() {
    for abort in [false, true] {
        let (producer, consumer) = fixtures("binary-producer-tree", "blocked-consumer-tree");
        let producer_spec = producer.spec();
        let consumer_spec = consumer.spec();
        let log = producer.0.join("pipeline.log");
        let (cancel, receiver) = watch::channel(false);
        let (events, _unread) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            run_pipeline(
                &producer_spec,
                &consumer_spec,
                receiver,
                events,
                &log,
                Duration::from_secs(30),
            )
            .await
        });
        let pids = [
            producer.pid(0).await,
            producer.pid(1).await,
            producer.pid(2).await,
            consumer.pid(0).await,
            consumer.pid(1).await,
            consumer.pid(2).await,
        ];
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.send(true).unwrap();
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(5), task)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(SupervisorError::Cancelled)
            ));
        }
        assert_dead(&pids).await;
    }
}

#[tokio::test]
async fn blocked_pipeline_timeout_stops_both_stages() {
    let (producer, consumer) = fixtures("binary-producer-tree", "blocked-consumer-tree");
    let (_cancel, receiver) = watch::channel(false);
    let (events, _unread) = mpsc::channel(1);
    let result = run_pipeline(
        &producer.spec(),
        &consumer.spec(),
        receiver,
        events,
        &producer.0.join("pipeline.log"),
        Duration::from_secs(2),
    )
    .await;
    assert!(matches!(result, Err(SupervisorError::Timeout)));
    assert_dead(&[
        producer.pid(0).await,
        producer.pid(1).await,
        producer.pid(2).await,
        consumer.pid(0).await,
        consumer.pid(1).await,
        consumer.pid(2).await,
    ])
    .await;
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
async fn blocked_consumer_applies_backpressure_after_one_fixed_buffer() {
    let count = Arc::new(AtomicU64::new(0));
    let reader = CountedReader(count.clone());
    let (writer, _unread) = tokio::io::duplex(1024);
    let task =
        tokio::spawn(async move { transfer_binary(reader, writer, &AtomicBool::new(false)).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while count.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(count.load(Ordering::SeqCst), PIPE_BYTES as u64);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[test]
fn public_supervisor_futures_remain_send_on_every_target() {
    fn assert_send<T: Send>(_: T) {}
    let spec = CommandSpec {
        executable: "unused".into(),
        args: vec![],
        cwd: None,
    };
    let (_cancel, receiver) = watch::channel(false);
    let (events, _unread) = mpsc::channel(1);
    assert_send(run_capture(
        &spec,
        receiver.clone(),
        1024,
        Duration::from_secs(1),
    ));
    assert_send(run_pipeline(
        &spec,
        &spec,
        receiver,
        events,
        Path::new("unused.log"),
        Duration::from_secs(1),
    ));
}
