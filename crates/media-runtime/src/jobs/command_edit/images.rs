//! Image reviews keep their native preparation task alive through confirmation.
use super::*;
use media_core::{ImageRequest, ImageResult};

pub(super) struct ImageState {
    result: Option<oneshot::Receiver<Result<ImageResult, AppError>>>,
    task: Option<tokio::task::JoinHandle<()>>,
    completion: watch::Receiver<bool>,
}

pub(super) async fn start(
    _manager: &JobManager,
    session: &Arc<Session>,
    request: ImageRequest,
) -> Result<(), AppError> {
    let (sender, result) = oneshot::channel();
    let (completed, completion) = watch::channel(false);
    struct Completion(watch::Sender<bool>);
    impl Drop for Completion {
        fn drop(&mut self) {
            self.0.send_replace(true);
        }
    }
    let completed = Completion(completed);
    let owner = session.clone();
    let pause = owner.image_pause.clone();
    let cancel = owner.cancel.subscribe();
    let task = tokio::spawn(async move {
        let _completed = completed;
        let result = crate::supervisor::with_pause(
            pause,
            crate::jobs::images::run_image_job_with_review(request, cancel, owner.clone()),
        )
        .await;
        owner.finish(result.as_ref().err().cloned());
        let _ = sender.send(result);
    });
    *session.image.lock().await = Some(ImageState {
        result: Some(result),
        task: Some(task),
        completion,
    });
    Ok(())
}

pub(super) async fn finish(
    session: &Arc<Session>,
    cancel: watch::Receiver<bool>,
) -> Result<ImageResult, AppError> {
    let mut result = session
        .image
        .lock()
        .await
        .as_mut()
        .and_then(|state| state.result.take())
        .ok_or_else(expired)?;
    let value = tokio::select! {
        biased;
        _ = wait_cancel(cancel) => {
            session.cancel();
            let _ = result.await;
            Err(canceled_review())
        }
        value = &mut result => value.unwrap_or_else(|_| Err(expired())),
    };
    join(session).await;
    value
}

pub(super) async fn join(session: &Arc<Session>) {
    let Some((task, mut completion)) = session
        .image
        .lock()
        .await
        .as_mut()
        .map(|state| (state.task.take(), state.completion.clone()))
    else {
        return;
    };
    if let Some(task) = task {
        let _ = task.await;
    } else {
        while !*completion.borrow_and_update() {
            if completion.changed().await.is_err() {
                break;
            }
        }
    }
}
