use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager;

pub const LABEL: &str = "quick-convert";

// Reconnecting one webview must not replace another webview's progress stream.
#[derive(Default)]
pub struct JobSubscriptions(Mutex<HashMap<String, Arc<AtomicBool>>>);

impl JobSubscriptions {
    pub fn start(&self, label: &str) -> Arc<AtomicBool> {
        let active = Arc::new(AtomicBool::new(true));
        let mut subscriptions = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = subscriptions.insert(label.to_owned(), active.clone()) {
            previous.store(false, Ordering::SeqCst);
        }
        active
    }

    pub fn stop(&self, label: &str) {
        if let Some(active) = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(label)
        {
            active.store(false, Ordering::SeqCst);
        }
    }
}

pub async fn open(app: tauri::AppHandle) -> Result<(), media_core::AppError> {
    // Serialize concurrent start/reopen requests so they share one native window.
    static OPEN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = OPEN.lock().await;
    let result = (|| -> tauri::Result<()> {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.show()?;
            window.unminimize()?;
            return window.set_focus();
        }
        let mut builder = tauri::WebviewWindowBuilder::new(
            &app,
            LABEL,
            tauri::WebviewUrl::App("index.html?window=quick-convert".into()),
        )
        .title("Quick Convert — jesses")
        .inner_size(900.0, 760.0)
        .min_inner_size(360.0, 360.0)
        .center()
        .theme(Some(tauri::Theme::Light));
        if let Some(directory) = app.state::<crate::paths::AppPaths>().webview_data_dir() {
            builder = builder.data_directory(directory);
        }
        builder.build()?;
        Ok(())
    })();
    result
        .map_err(|error| media_core::AppError::new("ENCODE_WINDOW_FAILED", error.to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_receive_independent_subscriptions() {
        let subscriptions = JobSubscriptions::default();
        let main = subscriptions.start("main");
        let monitor = subscriptions.start(LABEL);
        assert!(main.load(Ordering::SeqCst));
        assert!(monitor.load(Ordering::SeqCst));
        let reloaded = subscriptions.start(LABEL);
        assert!(!monitor.load(Ordering::SeqCst));
        assert!(main.load(Ordering::SeqCst));
        assert!(reloaded.load(Ordering::SeqCst));
        subscriptions.stop(LABEL);
        assert!(!reloaded.load(Ordering::SeqCst));
        assert!(main.load(Ordering::SeqCst));
        let reopened = subscriptions.start(LABEL);
        assert!(reopened.load(Ordering::SeqCst));
        assert!(!monitor.load(Ordering::SeqCst));
    }
}
