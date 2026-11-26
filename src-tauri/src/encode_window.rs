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

/// Shrink a window that is larger than its monitor's work area and move it
/// fully onto that area. A 1920×1080 laptop at 150 % offers 1280×720 logical
/// pixels, less than the windows' configured sizes, and the footer with Run
/// would otherwise open below the screen edge.
pub(crate) fn fit_to_work_area(window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let area = monitor.work_area();
    let (Ok(inner), Ok(outer), Ok(position)) = (
        window.inner_size(),
        window.outer_size(),
        window.outer_position(),
    ) else {
        return;
    };
    let frame_width = outer.width.saturating_sub(inner.width);
    let frame_height = outer.height.saturating_sub(inner.height);
    let width = inner.width.min(area.size.width.saturating_sub(frame_width));
    let height = inner
        .height
        .min(area.size.height.saturating_sub(frame_height));
    if (width, height) != (inner.width, inner.height) {
        let _ = window.set_size(tauri::PhysicalSize::new(width, height));
    }
    let outer_width = i64::from(width + frame_width);
    let outer_height = i64::from(height + frame_height);
    let left = i64::from(area.position.x);
    let top = i64::from(area.position.y);
    let x = i64::from(position.x).clamp(
        left,
        (left + i64::from(area.size.width) - outer_width).max(left),
    );
    let y = i64::from(position.y).clamp(
        top,
        (top + i64::from(area.size.height) - outer_height).max(top),
    );
    if (x, y) != (i64::from(position.x), i64::from(position.y)) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
}

pub async fn open(app: tauri::AppHandle, focus: bool) -> Result<(), media_core::AppError> {
    // Serialize concurrent start/reopen requests so they share one native window.
    static OPEN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = OPEN.lock().await;
    let result = (|| -> tauri::Result<()> {
        if let Some(window) = app.get_webview_window(LABEL) {
            if focus {
                window.show()?;
                window.unminimize()?;
                return window.set_focus();
            }
            return Ok(());
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
        .focused(focus)
        .theme(Some(tauri::Theme::Light));
        if let Some(directory) = app.state::<crate::paths::AppPaths>().webview_data_dir() {
            builder = builder.data_directory(directory);
        }
        fit_to_work_area(&builder.build()?);
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
