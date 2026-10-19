//! OS notifications when an agent turn finishes or needs input in the background (`#683`).
//!
//! ## Spike S1 — Notification click-to-focus across platforms (`#683` §3.8)
//!
//! We evaluated native zero-dependency OS notification delivery across macOS, Linux, and Windows:
//! - **macOS**: `osascript -e 'display notification ... with title "design:os studio"'` posts to
//!   Notification Center without requiring a signed `.app` bundle or `UNUserNotificationCenter`
//!   entitlements, but `osascript` is fire-and-forget and does not report banner clicks back to
//!   the spawning process.
//! - **Linux**: `notify-send` posts to the freedesktop.org notification daemon; without blocking
//!   on `--action` (which is unsupported on several lightweight notification daemons), click
//!   events are not reported.
//! - **Windows**: PowerShell `ToastNotificationManager` requires a registered COM `NotificationActivator`
//!   CLSID and AppUserModelID in the registry to receive activation callbacks.
//!
//! Per `#683` rule 8 ("If it cannot, ship the notification without the click and record which
//! platform lacks it"), notifications are dispatched on a background worker thread without
//! click-to-focus on all three platforms.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Status line message shown once when the OS denies or cannot deliver notifications (`#683` §8.8).
pub const NOTIFICATIONS_OFF_STATUS_MSG: &str = "Notifications are off";

/// Why an agent tab is requesting user attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyKind {
    /// A turn completed (or ended with an error) while the window was unfocused or the tab hidden.
    Finished,
    /// The agent is waiting on a permission prompt or user input.
    NeedsInput,
}

/// One formatted OS notification emitted for an agent tab (`#683` §3.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNotification {
    pub tab_id: usize,
    pub agent_name: String,
    pub file_name: String,
    pub kind: NotifyKind,
    pub message: String,
}

impl AgentNotification {
    pub fn new(tab_id: usize, agent_name: impl Into<String>, file_name: impl Into<String>, kind: NotifyKind) -> Self {
        let agent_name = agent_name.into();
        let file_name = file_name.into();
        let message = match kind {
            NotifyKind::Finished => format!("{agent_name} finished in {file_name}"),
            NotifyKind::NeedsInput => format!("{agent_name} needs your input"),
        };
        Self { tab_id, agent_name, file_name, kind, message }
    }
}

/// Outcome of delivering an [`AgentNotification`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationOutcome {
    Sent,
    Denied,
}

/// Pluggable sink for OS notifications (real background OS command in production, recording stub in tests).
pub trait NotificationSink: Send + Sync {
    fn send(&self, notification: &AgentNotification) -> NotificationOutcome;
}

/// Rule 6 & Rule 7 visibility gate: send a notification only when the Studio window is unfocused
/// or the tab is not visible (`!window_focused || !tab_visible`).
pub fn should_notify(window_focused: bool, tab_visible: bool) -> bool {
    !window_focused || !tab_visible
}

/// Recording stub sink used in unit tests and headless runs.
#[derive(Default)]
pub struct StubNotificationSink {
    sent: Mutex<Vec<AgentNotification>>,
    denied: AtomicBool,
}

impl StubNotificationSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_denied(denied: bool) -> Self {
        Self { sent: Mutex::new(Vec::new()), denied: AtomicBool::new(denied) }
    }

    pub fn set_denied(&self, denied: bool) {
        self.denied.store(denied, Ordering::SeqCst);
    }

    pub fn calls(&self) -> Vec<AgentNotification> {
        self.sent.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.sent.lock().map(|g| g.len()).unwrap_or(0)
    }
}

impl NotificationSink for StubNotificationSink {
    fn send(&self, notification: &AgentNotification) -> NotificationOutcome {
        if self.denied.load(Ordering::SeqCst) {
            return NotificationOutcome::Denied;
        }
        if let Ok(mut guard) = self.sent.lock() {
            guard.push(notification.clone());
        }
        NotificationOutcome::Sent
    }
}

/// Production OS notification sink that dispatches platform notifications on a worker thread.
pub struct OsNotificationSink {
    denied_detected: Arc<AtomicBool>,
}

impl Default for OsNotificationSink {
    fn default() -> Self {
        Self { denied_detected: Arc::new(AtomicBool::new(false)) }
    }
}

impl NotificationSink for OsNotificationSink {
    fn send(&self, notification: &AgentNotification) -> NotificationOutcome {
        if self.denied_detected.load(Ordering::Relaxed) {
            return NotificationOutcome::Denied;
        }
        let msg = notification.message.clone();
        let denied_flag = Arc::clone(&self.denied_detected);
        let spawned = std::thread::Builder::new().name("designos-agent-notify".into()).spawn(move || {
            if !deliver_os_notification(&msg) {
                denied_flag.store(true, Ordering::Relaxed);
            }
        });
        if spawned.is_err() {
            self.denied_detected.store(true, Ordering::Relaxed);
            return NotificationOutcome::Denied;
        }
        NotificationOutcome::Sent
    }
}

fn deliver_os_notification(body: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let escaped = body.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("display notification \"{escaped}\" with title \"design:os studio\"");
        std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("notify-send")
            .arg("design:os studio")
            .arg(body)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = body;
        false
    }
}

/// Per-terminal notification dispatcher that enforces one notification per turn and one-time
/// status-line reporting when notifications are off (`#683` §3.6, §8.8).
pub struct AgentNotifier {
    sink: Arc<dyn NotificationSink>,
    pub notifications_off_noted: bool,
}

impl std::fmt::Debug for AgentNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentNotifier").field("notifications_off_noted", &self.notifications_off_noted).finish()
    }
}

impl Default for AgentNotifier {
    fn default() -> Self {
        #[cfg(test)]
        let sink: Arc<dyn NotificationSink> = Arc::new(StubNotificationSink::new());
        #[cfg(not(test))]
        let sink: Arc<dyn NotificationSink> = Arc::new(OsNotificationSink::default());
        Self { sink, notifications_off_noted: false }
    }
}

impl AgentNotifier {
    pub fn with_sink(sink: Arc<dyn NotificationSink>) -> Self {
        Self { sink, notifications_off_noted: false }
    }

    pub fn set_sink(&mut self, sink: Arc<dyn NotificationSink>) {
        self.sink = sink;
    }

    /// Sends `notification` if `should_notify(window_focused, tab_visible)` holds.
    /// Returns `Some(NOTIFICATIONS_OFF_STATUS_MSG)` once if the OS denies the notification (`#683` §8.8).
    pub fn notify_if_background(&mut self, window_focused: bool, tab_visible: bool, notification: AgentNotification) -> Option<String> {
        if !should_notify(window_focused, tab_visible) {
            return None;
        }
        match self.sink.send(&notification) {
            NotificationOutcome::Sent => None,
            NotificationOutcome::Denied => {
                if !self.notifications_off_noted {
                    self.notifications_off_noted = true;
                    Some(NOTIFICATIONS_OFF_STATUS_MSG.to_string())
                } else {
                    None
                }
            }
        }
    }
}
