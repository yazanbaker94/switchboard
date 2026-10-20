//! Local workspace preferences only. Browser profiles and message data are never read.

use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub mode: String,
    pub last_single_account: String,
    pub compact_sidebar: bool,
    pub summon_shortcut: String,
    pub next_unread_shortcut: String,
    pub window: WindowSize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

impl Default for WindowSize {
    fn default() -> Self {
        Self {
            width: 1440.0,
            height: 900.0,
            maximized: false,
        }
    }
}

impl WindowSize {
    pub fn observe(&mut self, width: f64, height: f64, maximized: bool, minimized: bool) {
        if minimized || !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return;
        }
        self.maximized = maximized;
        if !maximized {
            self.width = width;
            self.height = height;
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            mode: "personal".into(),
            last_single_account: "personal".into(),
            compact_sidebar: false,
            summon_shortcut: "Ctrl+Alt+W".into(),
            next_unread_shortcut: "Ctrl+Alt+U".into(),
            window: WindowSize::default(),
        }
    }
}

impl Preferences {
    fn validated(mut self) -> Self {
        if !matches!(self.mode.as_str(), "personal" | "work" | "split") {
            self.mode = "personal".into();
        }
        if !matches!(self.last_single_account.as_str(), "personal" | "work") {
            self.last_single_account = "personal".into();
        }
        if !self.window.width.is_finite() || !(400.0..=10000.0).contains(&self.window.width) {
            self.window.width = 1440.0;
        }
        if !self.window.height.is_finite() || !(300.0..=10000.0).contains(&self.window.height) {
            self.window.height = 900.0;
        }
        if !matches!(
            self.summon_shortcut.as_str(),
            "" | "Ctrl+Alt+W" | "Ctrl+Alt+Space"
        ) {
            self.summon_shortcut = "Ctrl+Alt+W".into();
        }
        if !matches!(
            self.next_unread_shortcut.as_str(),
            "" | "Ctrl+Alt+U" | "Ctrl+Alt+N"
        ) {
            self.next_unread_shortcut = "Ctrl+Alt+U".into();
        }
        self
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pending: bool,
    last_error: Option<String>,
}

enum WriteRequest {
    Update(Preferences),
    Flush(mpsc::SyncSender<Result<(), String>>),
}

pub struct Store {
    value: Mutex<Preferences>,
    sender: mpsc::Sender<WriteRequest>,
    status: Arc<Mutex<Status>>,
}

fn write_file(path: &std::path::Path, value: &Preferences) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}

impl Store {
    pub fn open(path: PathBuf) -> Self {
        let value = std::fs::metadata(&path)
            .ok()
            .filter(|metadata| metadata.len() <= 65536)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
            .unwrap_or_default()
            .validated();
        let status = Arc::new(Mutex::new(Status::default()));
        let worker_status = status.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("switchboard-preferences".into())
            .spawn(move || {
                let mut pending = None;
                loop {
                    let request = if pending.is_some() {
                        receiver.recv_timeout(Duration::from_millis(500))
                    } else {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    let acknowledgement = match request {
                        Ok(WriteRequest::Update(value)) => {
                            pending = Some(value);
                            if let Ok(mut status) = worker_status.lock() {
                                status.pending = true;
                            }
                            continue;
                        }
                        Ok(WriteRequest::Flush(acknowledgement)) => Some(acknowledgement),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            if let Some(value) = pending.take() {
                                let _ = write_file(&path, &value);
                            }
                            break;
                        }
                    };
                    let attempted = pending.is_some();
                    let result = pending
                        .as_ref()
                        .map_or(Ok(()), |value| write_file(&path, value));
                    if result.is_ok() {
                        pending = None;
                    }
                    if let Ok(mut status) = worker_status.lock() {
                        status.pending = pending.is_some();
                        if attempted {
                            status.last_error = result.as_ref().err().cloned();
                        }
                    }
                    if let Some(acknowledgement) = acknowledgement {
                        let _ = acknowledgement.send(result);
                    }
                }
            });
        if worker.is_err() {
            if let Ok(mut status) = status.lock() {
                status.last_error = Some("The local preference writer could not start".into());
            }
        }
        Self {
            value: Mutex::new(value),
            sender,
            status,
        }
    }

    pub fn snapshot(&self) -> Preferences {
        self.value
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub fn update(&self, change: impl FnOnce(&mut Preferences)) {
        if let Ok(mut value) = self.value.lock() {
            let previous = value.clone();
            change(&mut value);
            if *value == previous {
                return;
            }
            if let Ok(mut status) = self.status.lock() {
                status.pending = true;
            }
            if self
                .sender
                .send(WriteRequest::Update(value.clone()))
                .is_err()
            {
                if let Ok(mut status) = self.status.lock() {
                    status.pending = false;
                    status.last_error = Some("The local preference writer is unavailable".into());
                }
            }
        }
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn flush(&self) {
        let (sender, receiver) = mpsc::sync_channel(1);
        if self.sender.send(WriteRequest::Flush(sender)).is_ok() {
            let _ = receiver.recv_timeout(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximize_and_minimize_preserve_the_last_useful_normal_window_size() {
        let mut size = WindowSize::default();
        size.observe(1200.0, 720.0, false, false);
        size.observe(1536.0, 800.0, true, false);
        size.observe(0.0, 0.0, false, true);
        assert_eq!(size.width, 1200.0);
        assert_eq!(size.height, 720.0);
        assert!(size.maximized);
        size.observe(1200.0, 720.0, false, false);
        assert!(!size.maximized);
    }

    #[test]
    fn invalid_preferences_fall_back_without_changing_account_profiles() {
        let mut value = Preferences::default();
        value.mode = "foreign".into();
        value.last_single_account = "split".into();
        value.summon_shortcut = "Alt+F4".into();
        value.window.width = f64::INFINITY;
        value.window.height = 0.0;
        assert_eq!(value.validated(), Preferences::default());
    }

    #[test]
    fn preference_changes_flush_and_reopen_with_only_workspace_fields() {
        let dir =
            std::env::temp_dir().join(format!("switchboard-preferences-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workspace.json");
        let store = Store::open(path.clone());
        store.update(|value| {
            value.mode = "split".into();
            value.last_single_account = "work".into();
            value.compact_sidebar = true;
            value.window.width = 1200.0;
            value.window.maximized = true;
        });
        store.flush();
        store.update(|value| value.window.width = 1300.0);
        store.flush();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("unread"));
        assert!(!saved.contains("title"));
        assert_eq!(
            serde_json::from_str::<Preferences>(&saved)
                .unwrap()
                .window
                .width,
            1300.0
        );
        let reopened = Store::open(path.clone());
        assert_eq!(store.snapshot(), reopened.snapshot());
        drop(store);
        drop(reopened);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn failed_writes_keep_the_latest_dirty_value_for_a_later_flush() {
        let dir = std::env::temp_dir().join(format!(
            "switchboard-preferences-retry-{}",
            std::process::id()
        ));
        let path = dir.join("workspace.json");
        let store = Store::open(path.clone());
        store.update(|value| value.compact_sidebar = true);
        store.flush();
        assert!(store.status().pending);
        assert!(store.status().last_error.is_some());
        store.flush();
        assert!(store.status().last_error.is_some());
        std::fs::create_dir_all(&dir).unwrap();
        store.flush();
        assert!(!store.status().pending);
        assert!(store.status().last_error.is_none());
        assert!(
            serde_json::from_slice::<Preferences>(&std::fs::read(&path).unwrap())
                .unwrap()
                .compact_sidebar
        );
        drop(store);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
