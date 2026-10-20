use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    webview::{NewWindowResponse, PageLoadEvent},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Rect, Webview, WebviewBuilder,
    WebviewUrl, WindowBuilder, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

const SIDEBAR: f64 = 232.0;
const COMPACT_SIDEBAR: f64 = 72.0;
const HEADER: f64 = 76.0;
const ACCOUNTS: [&str; 2] = ["personal", "work"];
const SPLIT_CSS_WIDTH: f64 = 900.0;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Personal,
    Work,
    Split,
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Personal => "personal",
        Mode::Work => "work",
        Mode::Split => "split",
    }
}

fn saved_mode(mode: &str) -> Mode {
    match mode {
        "work" => Mode::Work,
        "split" => Mode::Split,
        _ => Mode::Personal,
    }
}

struct Workspace {
    mode: Mutex<Mode>,
    last_single_account: Mutex<Mode>,
    compact_sidebar: Mutex<bool>,
    unread: Mutex<[Option<u32>; 2]>,
    tray_status: Mutex<TrayStatus>,
    tray_pending: AtomicBool,
    overlay: Mutex<bool>,
    loaded: Mutex<[bool; 2]>,
    downloads: Mutex<HashSet<PathBuf>>,
    labels: Mutex<[String; 2]>,
    layout: Mutex<LayoutState>,
    foreground: Mutex<ForegroundStatus>,
    layout_pending: AtomicBool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrayStatus {
    icon_mask: Option<u8>,
    tooltip_utf16_units: usize,
    refreshes: u64,
    last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
}

impl Geometry {
    fn rect(self) -> Rect {
        Rect {
            position: LogicalPosition::new(self.x, self.y).into(),
            size: LogicalSize::new(self.width, self.height).into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ViewLayout {
    geometry: Option<Geometry>,
    zoom: Option<f64>,
    visible: Option<bool>,
}

impl ViewLayout {
    fn changes(self, previous: Self) -> Self {
        Self {
            geometry: self
                .geometry
                .filter(|value| previous.geometry != Some(*value)),
            zoom: self.zoom.filter(|value| {
                !previous
                    .zoom
                    .is_some_and(|old| (old - value).abs() < 0.000001)
            }),
            visible: self
                .visible
                .filter(|value| previous.visible != Some(*value)),
        }
    }

    fn remember(&mut self, changes: Self) {
        if changes.geometry.is_some() {
            self.geometry = changes.geometry;
        }
        if changes.zoom.is_some() {
            self.zoom = changes.zoom;
        }
        if changes.visible.is_some() {
            self.visible = changes.visible;
        }
    }
}

#[derive(Default)]
struct LayoutState {
    views: [ViewLayout; 2],
    title: Option<String>,
    metrics: LayoutMetrics,
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct LayoutMetrics {
    passes: u64,
    preserved_geometry_passes: u64,
    bounds_requests: u64,
    zoom_requests: u64,
    visibility_requests: u64,
    title_requests: u64,
    last_native_requests: u64,
    last_native_work_us: u64,
}

impl LayoutMetrics {
    fn record(&mut self, pass: Self) {
        self.passes += pass.passes;
        self.preserved_geometry_passes += pass.preserved_geometry_passes;
        self.bounds_requests += pass.bounds_requests;
        self.zoom_requests += pass.zoom_requests;
        self.visibility_requests += pass.visibility_requests;
        self.title_requests += pass.title_requests;
        self.last_native_requests = pass.last_native_requests;
        self.last_native_work_us = pass.last_native_work_us;
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForegroundStatus {
    attempts: u64,
    os_accepted: bool,
    actual_foreground: bool,
    recent: VecDeque<ForegroundAttempt>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForegroundAttempt {
    source: &'static str,
    generated_at_ms: u128,
    os_accepted: bool,
    actual_foreground: bool,
}

#[derive(Clone, Serialize)]
struct AccountLoad {
    account: String,
    state: &'static str,
}

#[derive(Clone, Serialize)]
struct AccountUnread {
    account: String,
    count: Option<u32>,
}

fn require_shell(view: &Webview) -> Result<(), String> {
    if view.label() == "shell" {
        Ok(())
    } else {
        Err("Only the local app shell can control this workspace".into())
    }
}

fn bounds_with_sidebar(mode: Mode, width: f64, height: f64, account: &str, sidebar: f64) -> Rect {
    let available = (width - sidebar).max(1.0);
    let (x, w) = if mode == Mode::Split {
        let half = (available - 4.0).max(2.0) / 2.0;
        (
            sidebar + if account == "work" { half + 4.0 } else { 0.0 },
            half,
        )
    } else {
        (sidebar, available)
    };
    Rect {
        position: LogicalPosition::new(x, HEADER).into(),
        size: LogicalSize::new(w, (height - HEADER).max(1.0)).into(),
    }
}

#[cfg(test)]
fn bounds(mode: Mode, width: f64, height: f64, account: &str) -> Rect {
    bounds_with_sidebar(mode, width, height, account, SIDEBAR)
}

fn visible(mode: Mode, account: &str) -> bool {
    mode == Mode::Split
        || (mode == Mode::Personal && account == "personal")
        || (mode == Mode::Work && account == "work")
}

fn content_zoom(mode: Mode, pane_width: f64) -> f64 {
    if mode != Mode::Split {
        return 1.0;
    }
    if !pane_width.is_finite() {
        return 0.4;
    }
    (pane_width / SPLIT_CSS_WIDTH).clamp(0.4, 1.0)
}

fn desired_view_layout_with_sidebar(
    mode: Mode,
    width: f64,
    height: f64,
    scale: f64,
    minimized: bool,
    overlay: bool,
    account: &str,
    sidebar: f64,
) -> ViewLayout {
    let geometry = (!minimized && width > 0.0 && height > 0.0).then(|| {
        let rect = bounds_with_sidebar(mode, width, height, account, sidebar);
        let position = rect.position.to_logical::<f64>(scale);
        let size = rect.size.to_logical::<f64>(scale);
        Geometry {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale,
        }
    });
    ViewLayout {
        geometry,
        zoom: geometry.map(|geometry| content_zoom(mode, geometry.width)),
        visible: Some(!overlay && visible(mode, account)),
    }
}

#[cfg(test)]
fn desired_view_layout(
    mode: Mode,
    width: f64,
    height: f64,
    scale: f64,
    minimized: bool,
    overlay: bool,
    account: &str,
) -> ViewLayout {
    desired_view_layout_with_sidebar(
        mode, width, height, scale, minimized, overlay, account, SIDEBAR,
    )
}

fn native_layout_request(
    metrics: &mut LayoutMetrics,
    action: impl FnOnce() -> tauri::Result<()>,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let result = action();
    // Measures host requests only, never browser painting or message latency.
    metrics.last_native_work_us = metrics
        .last_native_work_us
        .saturating_add(started.elapsed().as_micros().min(u64::MAX as u128) as u64);
    metrics.last_native_requests += 1;
    result.map_err(|error| error.to_string())
}

fn apply_layout_locked(app: &AppHandle, mode: Mode) -> Result<(), String> {
    let window = app
        .get_window("main")
        .ok_or("Workspace window is unavailable")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let size = window
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(scale);
    let minimized = window.is_minimized().map_err(|e| e.to_string())?;
    let overlay = *app
        .state::<Workspace>()
        .overlay
        .lock()
        .map_err(|e| e.to_string())?;
    let state = app.state::<Workspace>();
    let sidebar = if *state.compact_sidebar.lock().map_err(|e| e.to_string())? {
        COMPACT_SIDEBAR
    } else {
        SIDEBAR
    };
    let maximized = window.is_maximized().map_err(|error| error.to_string())?;
    app.state::<crate::preferences::Store>()
        .update(|preferences| {
            preferences
                .window
                .observe(size.width, size.height, maximized, minimized);
        });
    let mut metrics = LayoutMetrics {
        passes: 1,
        preserved_geometry_passes: u64::from(minimized || size.width == 0.0 || size.height == 0.0),
        ..Default::default()
    };
    let result = (|| {
        for (index, account) in ACCOUNTS.into_iter().enumerate() {
            if let Some(view) = app.get_webview(account) {
                let desired = desired_view_layout_with_sidebar(
                    mode,
                    size.width,
                    size.height,
                    scale,
                    minimized,
                    overlay,
                    account,
                    sidebar,
                );
                let previous = state.layout.lock().map_err(|e| e.to_string())?.views[index];
                let changes = desired.changes(previous);
                if let Some(geometry) = changes.geometry {
                    metrics.bounds_requests += 1;
                    native_layout_request(&mut metrics, || view.set_bounds(geometry.rect()))?;
                }
                if let Some(zoom) = changes.zoom {
                    metrics.zoom_requests += 1;
                    native_layout_request(&mut metrics, || view.set_zoom(zoom))?;
                }
                if let Some(visible) = changes.visible {
                    metrics.visibility_requests += 1;
                    native_layout_request(&mut metrics, || {
                        if visible {
                            view.show()
                        } else {
                            view.hide()
                        }
                    })?;
                }
                state.layout.lock().map_err(|e| e.to_string())?.views[index].remember(changes);
            }
        }
        let label = match mode {
            Mode::Personal => account_label(app, "personal"),
            Mode::Work => account_label(app, "work"),
            Mode::Split => format!(
                "{} + {}",
                account_label(app, "personal"),
                account_label(app, "work")
            ),
        };
        let title = format!("Switchboard — {label}");
        let changed = state
            .layout
            .lock()
            .map_err(|e| e.to_string())?
            .title
            .as_ref()
            != Some(&title);
        if changed {
            metrics.title_requests += 1;
            native_layout_request(&mut metrics, || window.set_title(&title))?;
            state.layout.lock().map_err(|e| e.to_string())?.title = Some(title);
        }
        Ok(())
    })();
    state
        .layout
        .lock()
        .map_err(|e| e.to_string())?
        .metrics
        .record(metrics);
    result
}

fn layout_status(app: &AppHandle) -> Option<LayoutMetrics> {
    app.state::<Workspace>()
        .layout
        .lock()
        .ok()
        .map(|layout| layout.metrics)
}

fn apply_layout(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<Workspace>();
    let mode = *state.mode.lock().map_err(|e| e.to_string())?;
    apply_layout_locked(app, mode)
}

fn schedule_layout(app: &AppHandle) {
    if app
        .state::<Workspace>()
        .layout_pending
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let app = app.clone();
    // Restoring during WM_COPYDATA can synchronously emit Resized. Leave the
    // input callback before touching WebView2, and coalesce resize bursts.
    tauri::async_runtime::spawn(async move {
        let handle = app.clone();
        if app
            .run_on_main_thread(move || {
                handle
                    .state::<Workspace>()
                    .layout_pending
                    .store(false, Ordering::Release);
                let _ = apply_layout(&handle);
            })
            .is_err()
        {
            app.state::<Workspace>()
                .layout_pending
                .store(false, Ordering::Release);
        }
    });
}

fn switch_mode(app: &AppHandle, mode: Mode, focus: bool) -> Result<(), String> {
    let state = app.state::<Workspace>();
    *state.overlay.lock().map_err(|error| error.to_string())? = false;
    *state.mode.lock().map_err(|e| e.to_string())? = mode;
    if mode != Mode::Split {
        *state
            .last_single_account
            .lock()
            .map_err(|e| e.to_string())? = mode;
    }
    let last_single = *state
        .last_single_account
        .lock()
        .map_err(|e| e.to_string())?;
    app.state::<crate::preferences::Store>()
        .update(|preferences| {
            preferences.mode = mode_name(mode).into();
            preferences.last_single_account = mode_name(last_single).into();
        });
    apply_layout_locked(app, mode)?;
    let _ = app.emit_to("shell", "workspace-mode", mode);
    if focus {
        focus_account(app, mode);
    }
    Ok(())
}

fn focus_account(app: &AppHandle, mode: Mode) {
    let account = if mode == Mode::Work {
        "work"
    } else {
        "personal"
    };
    if let Some(view) = app.get_webview(account) {
        let _ = view.set_focus();
    }
}

async fn on_main<F>(app: AppHandle, action: F) -> Result<(), String>
where
    F: FnOnce(&AppHandle) -> Result<(), String> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = sender.send(action(&handle));
    })
    .map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())?
}

fn dispatch_switch(app: AppHandle, mode: Mode, focus: bool) {
    // Leave WebView2 accelerator callbacks before manipulating their controllers.
    std::thread::spawn(move || {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = switch_mode(&handle, mode, focus);
        });
    });
}

pub(crate) fn account_label(app: &AppHandle, account: &str) -> String {
    let index = if account == "work" { 1 } else { 0 };
    app.state::<Workspace>()
        .labels
        .lock()
        .map(|labels| labels[index].clone())
        .unwrap_or_else(|_| {
            if index == 0 {
                "Personal".into()
            } else {
                "Work".into()
            }
        })
}

/// Runs only on the UI thread, after leaving notification/COM callbacks.
pub(crate) fn activate_account(app: &AppHandle, account: &str) -> Result<(), String> {
    let mode = match account {
        "personal" => Mode::Personal,
        "work" => Mode::Work,
        _ => return Err("Unknown account".into()),
    };
    switch_mode(app, mode, true)
}

/// Request foreground once on the UI thread. Windows retains its normal focus policy.
pub(crate) fn request_foreground(app: &AppHandle, source: &'static str) -> Result<(), String> {
    let window = app
        .get_window("main")
        .ok_or("Workspace window is unavailable")?;
    #[cfg(windows)]
    let (os_accepted, actual_foreground) = {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        let was_iconic = unsafe { IsIconic(hwnd).as_bool() };
        // Keep Tao's visibility/minimize flags synchronized so a later tray
        // hide still works. These methods do not use its set_focus fallback.
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        unsafe {
            // SW_RESTORE also unmaximizes, so use it only for an iconic window.
            let command = if was_iconic { SW_RESTORE } else { SW_SHOW };
            let _ = ShowWindow(hwnd, command);
            let accepted = SetForegroundWindow(hwnd).as_bool();
            (accepted, GetForegroundWindow() == hwnd)
        }
    };
    #[cfg(not(windows))]
    let (os_accepted, actual_foreground) = {
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        (
            false,
            window.is_focused().map_err(|error| error.to_string())?,
        )
    };
    let state = app.state::<Workspace>();
    let mut foreground = state.foreground.lock().map_err(|error| error.to_string())?;
    foreground.attempts = foreground.attempts.saturating_add(1);
    foreground.os_accepted = os_accepted;
    foreground.actual_foreground = actual_foreground;
    if foreground.recent.len() >= 8 {
        foreground.recent.pop_front();
    }
    foreground.recent.push_back(ForegroundAttempt {
        source,
        generated_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0),
        os_accepted,
        actual_foreground,
    });
    Ok(())
}

fn foreground_status(app: &AppHandle) -> Option<ForegroundStatus> {
    app.state::<Workspace>()
        .foreground
        .lock()
        .ok()
        .map(|status| status.clone())
}

fn should_request_immediate_foreground(arguments: &[String]) -> bool {
    arguments.is_empty()
        || (arguments.len() == 1 && crate::notifications::is_activation_route(&arguments[0]))
}

fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let counts = *app
        .state::<Workspace>()
        .unread
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let label = |index: usize| {
        let name = account_label(app, ACCOUNTS[index]);
        match counts[index] {
            Some(count) if count > 0 => format!("Open {name} ({count} unread)"),
            _ => format!("Open {name}"),
        }
    };
    let personal = MenuItem::with_id(app, "personal", label(0), true, None::<&str>)?;
    let work = MenuItem::with_id(app, "work", label(1), true, None::<&str>)?;
    let split = MenuItem::with_id(app, "split", "Show both accounts", true, None::<&str>)?;
    let next = MenuItem::with_id(
        app,
        "next-unread",
        "Next unread account",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Switchboard", true, None::<&str>)?;
    Menu::with_items(app, &[&personal, &work, &split, &next, &quit])
}

fn refresh_tray(app: &AppHandle) -> Result<(), String> {
    let Some(tray) = app.tray_by_id("switchboard") else {
        return Ok(());
    };
    let state = app.state::<Workspace>();
    let counts = *state.unread.lock().map_err(|error| error.to_string())?;
    let labels = state
        .labels
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let tooltip = crate::tray_state::tooltip(&labels, counts);
    tray.set_tooltip(Some(&tooltip))
        .map_err(|error| error.to_string())?;
    tray.set_menu(Some(tray_menu(app).map_err(|error| error.to_string())?))
        .map_err(|error| error.to_string())?;
    let mask = u8::from(counts[0].unwrap_or(0) > 0) | (u8::from(counts[1].unwrap_or(0) > 0) << 1);
    let previous = state
        .tray_status
        .lock()
        .map_err(|error| error.to_string())?
        .icon_mask;
    if previous != Some(mask) {
        if let Some(base) = app.default_window_icon() {
            tray.set_icon(Some(crate::tray_state::icon(base, mask)))
                .map_err(|error| error.to_string())?;
            state
                .tray_status
                .lock()
                .map_err(|error| error.to_string())?
                .icon_mask = Some(mask);
        }
    }
    let mut status = state
        .tray_status
        .lock()
        .map_err(|error| error.to_string())?;
    status.refreshes += 1;
    status.tooltip_utf16_units = tooltip.encode_utf16().count();
    status.last_error = None;
    Ok(())
}

fn tray_status(app: &AppHandle) -> TrayStatus {
    app.state::<Workspace>()
        .tray_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default()
}

fn workspace_status(app: &AppHandle) -> serde_json::Value {
    let state = app.state::<Workspace>();
    let mode = state
        .mode
        .lock()
        .map(|mode| *mode)
        .unwrap_or(Mode::Personal);
    let last_single = state
        .last_single_account
        .lock()
        .map(|mode| *mode)
        .unwrap_or(Mode::Personal);
    let compact = state
        .compact_sidebar
        .lock()
        .map(|compact| *compact)
        .unwrap_or(false);
    let unread = state
        .unread
        .lock()
        .map(|unread| *unread)
        .unwrap_or([None; 2]);
    let store = app.state::<crate::preferences::Store>();
    serde_json::json!({
        "version":env!("CARGO_PKG_VERSION"),"mode":mode,"lastSingleAccount":last_single,
        "compactSidebar":compact,"unread":{"personal":unread[0],"work":unread[1]},
        "shortcuts":crate::shortcuts::status(),"preferences":store.status(),"window":store.snapshot().window,
        "tray":tray_status(app),"foreground":foreground_status(app),"layout":layout_status(app)
    })
}

fn schedule_tray(app: &AppHandle) {
    if app
        .state::<Workspace>()
        .tray_pending
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let handle = app.clone();
        if app
            .run_on_main_thread(move || {
                handle
                    .state::<Workspace>()
                    .tray_pending
                    .store(false, Ordering::Release);
                if let Err(error) = refresh_tray(&handle) {
                    eprintln!("Tray refresh: {error}");
                    if let Ok(mut status) = handle.state::<Workspace>().tray_status.lock() {
                        status.last_error = Some("The tray could not update".into());
                    }
                }
            })
            .is_err()
        {
            app.state::<Workspace>()
                .tray_pending
                .store(false, Ordering::Release);
        }
    });
}

fn update_unread(app: &AppHandle, account: &str, count: Option<u32>) {
    let index = usize::from(account == "work");
    let changed = app
        .state::<Workspace>()
        .unread
        .lock()
        .map(|mut unread| {
            if unread[index] == count {
                return false;
            }
            unread[index] = count;
            true
        })
        .unwrap_or(false);
    if changed {
        let _ = app.emit_to(
            "shell",
            "account-unread",
            AccountUnread {
                account: account.into(),
                count,
            },
        );
        schedule_tray(app);
    }
}

pub(crate) fn handle_global_shortcut(app: &AppHandle, action: crate::shortcuts::Action) {
    if action == crate::shortcuts::Action::Summon {
        #[cfg(windows)]
        if let Some(window) = app.get_window("main") {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic};
            if let Ok(hwnd) = window.hwnd() {
                if unsafe { GetForegroundWindow() == hwnd && !IsIconic(hwnd).as_bool() } {
                    let _ = window.hide();
                    return;
                }
            }
        }
        let _ = request_foreground(app, "global-summon");
        return;
    }
    let state = app.state::<Workspace>();
    let mode = state
        .mode
        .lock()
        .map(|mode| *mode)
        .unwrap_or(Mode::Personal);
    let current = if mode == Mode::Split {
        state
            .last_single_account
            .lock()
            .map(|mode| *mode)
            .unwrap_or(Mode::Personal)
    } else {
        mode
    };
    let counts = state
        .unread
        .lock()
        .map(|unread| *unread)
        .unwrap_or([None; 2]);
    let target = crate::tray_state::next_unread(usize::from(current == Mode::Work), counts)
        .map(|index| saved_mode(ACCOUNTS[index]));
    if let Some(target) = target {
        let _ = switch_mode(app, target, false);
    } else {
        let _ = switch_mode(app, mode, false);
    }
    let _ = request_foreground(app, "global-next-unread");
    if let Some(target) = target {
        focus_account(app, target);
    }
}

#[tauri::command]
async fn set_labels(
    view: Webview,
    app: AppHandle,
    personal: String,
    work: String,
) -> Result<(), String> {
    require_shell(&view)?;
    let personal = personal.trim().to_string();
    let work = work.trim().to_string();
    for label in [&personal, &work] {
        if label.is_empty() || label.chars().count() > 28 || label.chars().any(char::is_control) {
            return Err("Account names must contain 1–28 printable characters".into());
        }
    }
    on_main(app, move |app| {
        *app.state::<Workspace>()
            .labels
            .lock()
            .map_err(|e| e.to_string())? = [personal, work];
        refresh_tray(app)?;
        apply_layout(app)
    })
    .await
}

#[tauri::command]
async fn set_compact_sidebar(view: Webview, app: AppHandle, compact: bool) -> Result<(), String> {
    require_shell(&view)?;
    on_main(app, move |app| {
        *app.state::<Workspace>()
            .compact_sidebar
            .lock()
            .map_err(|error| error.to_string())? = compact;
        app.state::<crate::preferences::Store>()
            .update(|preferences| preferences.compact_sidebar = compact);
        apply_layout(app)?;
        let _ = app.emit_to("shell", "workspace-compact", compact);
        Ok(())
    })
    .await
}

#[tauri::command]
async fn configure_shortcuts(
    view: Webview,
    app: AppHandle,
    summon: String,
    next_unread: String,
) -> Result<crate::shortcuts::Status, String> {
    require_shell(&view)?;
    crate::shortcuts::validate(&summon, &next_unread)?;
    // Plugin registration dispatches native work itself; leave the UI callback free.
    let status = crate::shortcuts::configure(&app, &summon, &next_unread)?;
    app.state::<crate::preferences::Store>()
        .update(|preferences| {
            preferences.summon_shortcut = if status.summon.registered {
                status.summon.accelerator.clone()
            } else {
                status.summon.requested_accelerator.clone()
            };
            preferences.next_unread_shortcut = if status.next_unread.registered {
                status.next_unread.accelerator.clone()
            } else {
                status.next_unread.requested_accelerator.clone()
            };
        });
    Ok(status)
}

#[tauri::command]
async fn set_layout(
    view: Webview,
    app: AppHandle,
    mode: Mode,
    width: f64,
    height: f64,
) -> Result<(), String> {
    require_shell(&view)?;
    if !width.is_finite() || !height.is_finite() {
        return Err("Invalid workspace size".into());
    }
    on_main(app, move |app| switch_mode(app, mode, false)).await
}

#[tauri::command]
async fn reload_account(view: Webview, app: AppHandle, account: String) -> Result<(), String> {
    require_shell(&view)?;
    if !ACCOUNTS.contains(&account.as_str()) {
        return Err("Unknown account".into());
    }
    on_main(app, move |app| {
        app.get_webview(&account)
            .ok_or("Account view is unavailable")?
            .reload()
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn set_overlay(view: Webview, app: AppHandle, open: bool) -> Result<(), String> {
    require_shell(&view)?;
    on_main(app, move |app| {
        *app.state::<Workspace>()
            .overlay
            .lock()
            .map_err(|e| e.to_string())? = open;
        apply_layout(app)
    })
    .await
}

#[tauri::command]
fn get_diagnostics(view: Webview, app: AppHandle) -> Result<serde_json::Value, String> {
    require_shell(&view)?;
    let base = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let loaded = *app
        .state::<Workspace>()
        .loaded
        .lock()
        .map_err(|e| e.to_string())?;
    let mut report = workspace_status(&app);
    report["personalProfile"] = serde_json::json!(base.join("profiles/personal"));
    report["workProfile"] = serde_json::json!(base.join("profiles/work"));
    report["loaded"] = serde_json::json!({"personal":loaded[0],"work":loaded[1]});
    report["accountViews"] = serde_json::json!(ACCOUNTS
        .iter()
        .filter(|account| app.get_webview(account).is_some())
        .count());
    Ok(report)
}

#[tauri::command]
async fn notification_permissions(
    view: Webview,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    require_shell(&view)?;
    crate::permissions::states(app).await
}

fn external_allowed(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "https" | "http" | "mailto" | "tel")
}

fn unique_download(directory: &Path, requested: &Path, reserved: &mut HashSet<PathBuf>) -> PathBuf {
    let filename = requested
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("download"));
    let desired = directory.join(filename);
    if !desired.exists() && reserved.insert(desired.clone()) {
        return desired;
    }
    let stem = desired.file_stem().unwrap_or_default().to_string_lossy();
    let ext = desired
        .extension()
        .map(|x| format!(".{}", x.to_string_lossy()))
        .unwrap_or_default();
    for n in 1.. {
        let candidate = directory.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(windows)]
fn install_shortcuts(view: &Webview, app: &AppHandle) -> tauri::Result<()> {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        },
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let app = app.clone();
    view.with_webview(move |platform| unsafe {
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                let mut key = 0;
                args.KeyEventKind(&mut kind)?;
                args.VirtualKey(&mut key)?;
                if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && GetKeyState(VK_CONTROL.0 as i32) < 0
                    && GetKeyState(VK_MENU.0 as i32) >= 0
                    && GetKeyState(VK_SHIFT.0 as i32) >= 0
                {
                    if key == 0x51 {
                        args.SetHandled(true)?;
                        let quit_app = app.clone();
                        std::thread::spawn(move || {
                            quit_app.exit(0);
                        });
                        return Ok(());
                    }
                    let mode = match key {
                        0x31 => Some(Mode::Personal),
                        0x32 => Some(Mode::Work),
                        0x33 => Some(Mode::Split),
                        _ => None,
                    };
                    if let Some(mode) = mode {
                        args.SetHandled(true)?;
                        let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                        args.PhysicalKeyStatus(&mut status)?;
                        if !status.WasKeyDown.as_bool() {
                            dispatch_switch(app.clone(), mode, true);
                        }
                    }
                }
            }
            Ok(())
        }));
        let mut token = 0;
        if let Err(error) = platform
            .controller()
            .add_AcceleratorKeyPressed(&handler, &mut token)
        {
            eprintln!("Keyboard shortcut setup: {error}");
        }
    })
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let base = app.path().app_local_data_dir()?;
    std::fs::create_dir_all(base.join("profiles"))?;
    let store = crate::preferences::Store::open(base.join("workspace.json"));
    let preferences = store.snapshot();
    app.manage(store);
    let initial_mode = saved_mode(&preferences.mode);
    *app.state::<Workspace>()
        .mode
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))? = initial_mode;
    *app.state::<Workspace>()
        .last_single_account
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))? =
        saved_mode(&preferences.last_single_account);
    *app.state::<Workspace>()
        .compact_sidebar
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))? = preferences.compact_sidebar;
    let notification_registration = match crate::notifications::register(app.handle()) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Account notification registration unavailable: {error}");
            false
        }
    };
    let window = WindowBuilder::new(app, "main")
        .title("Switchboard — Personal")
        .inner_size(preferences.window.width, preferences.window.height)
        .min_inner_size(980.0, 640.0)
        .prevent_overflow_with_margin(LogicalSize::new(32.0, 32.0))
        .center()
        .maximized(preferences.window.maximized)
        .build()?;
    let client_size = window
        .inner_size()?
        .to_logical::<f64>(window.scale_factor()?);
    let shell = window.add_child(
        WebviewBuilder::new("shell", WebviewUrl::App("index.html".into()))
            .data_directory(base.join("shell"))
            .auto_resize(),
        LogicalPosition::new(0.0, 0.0),
        client_size,
    )?;
    #[cfg(windows)]
    install_shortcuts(&shell, app.handle())?;
    for account in ACCOUNTS {
        let load_account = account.to_string();
        let title_account = account.to_string();
        let nav_app = app.handle().clone();
        let popup_app = app.handle().clone();
        let downloads = app.path().download_dir()?;
        let builder = WebviewBuilder::new(
            account,
            WebviewUrl::External("https://web.whatsapp.com/".parse()?),
        )
        .data_directory(base.join("profiles").join(account))
        .focused(false)
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            if url.scheme() == "https" && url.host_str() == Some("web.whatsapp.com") {
                true
            } else if url.as_str() == "about:blank" {
                true
            } else {
                if external_allowed(url) {
                    let _ = nav_app.opener().open_url(url.as_str(), None::<&str>);
                }
                false
            }
        })
        .on_new_window(move |url, _| {
            if external_allowed(&url) {
                let _ = popup_app.opener().open_url(url.as_str(), None::<&str>);
            }
            NewWindowResponse::Deny
        })
        .on_page_load(move |view, payload| {
            let state = if payload.event() == PageLoadEvent::Started {
                "loading"
            } else {
                "loaded"
            };
            if let Ok(mut loaded) = view.app_handle().state::<Workspace>().loaded.lock() {
                loaded[if load_account == "personal" { 0 } else { 1 }] = state == "loaded";
            }
            let _ = view.app_handle().emit_to(
                "shell",
                "account-load",
                AccountLoad {
                    account: load_account.clone(),
                    state,
                },
            );
        })
        .on_document_title_changed(move |view, title| {
            update_unread(
                view.app_handle(),
                &title_account,
                crate::tray_state::unread_count(&title),
            );
        })
        .on_download(move |view, event| {
            match event {
                tauri::webview::DownloadEvent::Requested { destination, .. } => {
                    if let Ok(mut reserved) =
                        view.app_handle().state::<Workspace>().downloads.lock()
                    {
                        *destination = unique_download(&downloads, destination, &mut reserved);
                    } else {
                        return false;
                    }
                }
                tauri::webview::DownloadEvent::Finished {
                    path: Some(path), ..
                } => {
                    if let Ok(mut reserved) =
                        view.app_handle().state::<Workspace>().downloads.lock()
                    {
                        reserved.remove(&path);
                    }
                }
                _ => (),
            }
            true
        });
        let initial_bounds = bounds_with_sidebar(
            initial_mode,
            client_size.width,
            client_size.height,
            account,
            if preferences.compact_sidebar {
                COMPACT_SIDEBAR
            } else {
                SIDEBAR
            },
        );
        let view = window.add_child(builder, initial_bounds.position, initial_bounds.size)?;
        if !visible(initial_mode, account) {
            view.hide()?;
        }
        #[cfg(windows)]
        install_shortcuts(&view, app.handle())?;
        crate::notifications::install(&view, app.handle(), account)?;
    }
    let menu = tray_menu(app.handle())?;
    TrayIconBuilder::with_id("switchboard")
        .icon(app.default_window_icon().expect("bundled app icon").clone())
        .tooltip("Switchboard — Personal + Work")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let mode = match event.id.as_ref() {
                "personal" => Mode::Personal,
                "work" => Mode::Work,
                "split" => Mode::Split,
                "next-unread" => {
                    handle_global_shortcut(app, crate::shortcuts::Action::NextUnread);
                    return;
                }
                "quit" => {
                    app.exit(0);
                    return;
                }
                _ => return,
            };
            let _ = switch_mode(app, mode, false);
            if let Some(window) = app.get_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            focus_account(app, mode);
        })
        .build(app)?;
    refresh_tray(app.handle()).map_err(std::io::Error::other)?;
    let handle = app.handle().clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Some(window) = handle.get_window("main") {
                let _ = window.hide();
            }
        }
        WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
            schedule_layout(&handle);
        }
        _ => (),
    });
    apply_layout(app.handle()).map_err(std::io::Error::other)?;
    shell.set_focus()?;
    if notification_registration {
        if let Err(error) = crate::toast_activation::start(app.handle()) {
            eprintln!("COM notification activation unavailable: {error}");
        }
    }
    if let Err(error) = crate::shortcuts::configure(
        app.handle(),
        &preferences.summon_shortcut,
        &preferences.next_unread_shortcut,
    ) {
        eprintln!("Global shortcut setup unavailable: {error}");
    }
    handle_arguments(app.handle(), std::env::args().skip(1));
    Ok(())
}

fn handle_arguments(app: &AppHandle, arguments: impl Iterator<Item = String>) {
    for argument in arguments {
        if crate::notifications::activate(app, &argument) {
            return;
        }
        #[cfg(windows)]
        if argument == "--diagnostics" {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let permissions = crate::permissions::states(app.clone()).await;
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Ok(base) = handle.path().app_local_data_dir() {
                        let generated_at_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_millis())
                            .unwrap_or(0);
                        let mut report = workspace_status(&handle);
                        report["generatedAtMs"] = serde_json::json!(generated_at_ms);
                        report["processId"] = serde_json::json!(std::process::id());
                        report["permissions"] = serde_json::json!(permissions.ok());
                        report["notifications"] = crate::notifications::status();
                        report["dataDirectory"] =
                            serde_json::json!(std::fs::canonicalize(&base).ok());
                        report["executable"] = serde_json::json!(std::env::current_exe().ok());
                        if let Ok(text) = serde_json::to_string_pretty(&report) {
                            let _ = std::fs::write(base.join("diagnostics.json"), text);
                        }
                    }
                });
            });
        }
        #[cfg(windows)]
        if let Some(account) = argument.strip_prefix("--test-notification=") {
            let account = account.to_owned();
            let app = app.clone();
            // Single-instance arguments arrive inside synchronous WM_COPYDATA.
            // Leave that callback before making WinRT notification calls.
            tauri::async_runtime::spawn(async move {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Err(error) = crate::notifications::test_toast(&handle, &account) {
                        eprintln!("Notification self-test: {error}");
                    }
                });
            });
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, arguments, _| {
            let arguments: Vec<_> = arguments.into_iter().skip(1).collect();
            // The protocol-launched process transferred its foreground grant.
            // Consume it now; defer all WebView2/WinRT work to handle_arguments.
            if should_request_immediate_foreground(&arguments) {
                let source = if arguments.is_empty() {
                    "reopen"
                } else {
                    "protocol-immediate"
                };
                let _ = request_foreground(app, source);
            }
            handle_arguments(app, arguments.into_iter());
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(crate::shortcuts::plugin())
        .manage(Workspace {
            mode: Mutex::new(Mode::Personal),
            last_single_account: Mutex::new(Mode::Personal),
            compact_sidebar: Mutex::new(false),
            unread: Mutex::new([None; 2]),
            tray_status: Mutex::new(TrayStatus::default()),
            tray_pending: AtomicBool::new(false),
            overlay: Mutex::new(false),
            loaded: Mutex::new([false; 2]),
            downloads: Mutex::new(HashSet::new()),
            labels: Mutex::new(["Personal".into(), "Work".into()]),
            layout: Mutex::new(LayoutState::default()),
            foreground: Mutex::new(ForegroundStatus::default()),
            layout_pending: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            set_layout,
            reload_account,
            set_overlay,
            get_diagnostics,
            set_labels,
            set_compact_sidebar,
            configure_shortcuts,
            crate::permissions::enable_notifications,
            notification_permissions
        ])
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("Switchboard could not start")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                crate::toast_activation::stop();
                if let Some(store) = app.try_state::<crate::preferences::Store>() {
                    store.flush();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_user_reopen_or_a_single_valid_route_requests_immediate_foreground() {
        assert!(should_request_immediate_foreground(&[]));
        for arguments in [
            vec!["--diagnostics".into()],
            vec!["--test-notification=work".into()],
            vec!["--unknown".into()],
            vec!["switchboard-wa://notification/work?id=1&extra=1".into()],
            vec![
                "--diagnostics".into(),
                "switchboard-wa://notification/work?id=1".into(),
            ],
        ] {
            assert!(!should_request_immediate_foreground(&arguments));
        }
        #[cfg(windows)]
        assert!(should_request_immediate_foreground(&[
            "switchboard-wa://notification/work?id=1".into()
        ]));
    }
    #[test]
    fn account_visibility_is_exclusive_except_split() {
        assert!(visible(Mode::Personal, "personal"));
        assert!(!visible(Mode::Personal, "work"));
        assert!(visible(Mode::Work, "work"));
        assert!(!visible(Mode::Work, "personal"));
        assert!(ACCOUNTS.into_iter().all(|a| visible(Mode::Split, a)));
    }
    #[test]
    fn compact_sidebar_expands_chat_bounds_and_overlay_does_not_change_saved_geometry() {
        for mode in [Mode::Personal, Mode::Work, Mode::Split] {
            for account in ACCOUNTS {
                let expanded = desired_view_layout_with_sidebar(
                    mode, 1440.0, 750.0, 1.25, false, false, account, SIDEBAR,
                );
                let compact = desired_view_layout_with_sidebar(
                    mode,
                    1440.0,
                    750.0,
                    1.25,
                    false,
                    false,
                    account,
                    COMPACT_SIDEBAR,
                );
                let expected = if mode == Mode::Split { 80.0 } else { 160.0 };
                assert_eq!(
                    compact.geometry.unwrap().width - expanded.geometry.unwrap().width,
                    expected
                );
                assert!(compact.zoom.unwrap() >= expanded.zoom.unwrap());
                let overlay = desired_view_layout_with_sidebar(
                    mode,
                    1440.0,
                    750.0,
                    1.25,
                    false,
                    true,
                    account,
                    COMPACT_SIDEBAR,
                );
                let changes = overlay.changes(compact);
                assert_eq!(changes.geometry, None);
                assert_eq!(changes.zoom, None);
                if compact.visible == Some(true) {
                    assert_eq!(changes.visible, Some(false));
                }
            }
        }
    }
    #[test]
    fn split_bounds_leave_shell_clear_and_do_not_overlap() {
        let left = bounds(Mode::Split, 1440.0, 900.0, "personal");
        let right = bounds(Mode::Split, 1440.0, 900.0, "work");
        assert_eq!(left.position.to_logical::<f64>(1.0).x, SIDEBAR);
        assert_eq!(right.position.to_logical::<f64>(1.0).x, 838.0);
        assert_eq!(left.size.to_logical::<f64>(1.0).width, 602.0);
        assert_eq!(left.position.to_logical::<f64>(1.0).y, HEADER);
    }
    #[test]
    fn split_zoom_fits_the_desktop_viewport_at_supported_window_sizes_and_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for width in [980.0, 1440.0, 1536.0, 2400.0] {
                let physical = LogicalSize::new(width, 800.0).to_physical::<u32>(scale);
                let logical = physical.to_logical::<f64>(scale);
                for account in ACCOUNTS {
                    let rect = bounds(Mode::Split, logical.width, logical.height, account);
                    let pane = rect.size.to_logical::<f64>(scale).width;
                    let zoom = content_zoom(Mode::Split, pane);
                    assert!((0.4..=1.0).contains(&zoom));
                    assert!(pane / zoom >= SPLIT_CSS_WIDTH - 0.000001);
                    if pane >= SPLIT_CSS_WIDTH {
                        assert_eq!(zoom, 1.0);
                    }
                }
            }
        }
        for width in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY, -1.0, 0.0, 1.0] {
            assert!((0.4..=1.0).contains(&content_zoom(Mode::Split, width)));
        }
    }
    #[test]
    fn single_account_modes_reset_both_visible_and_hidden_views_to_normal_zoom() {
        for mode in [Mode::Personal, Mode::Work] {
            for account in ACCOUNTS {
                let rect = bounds(mode, 980.0, 640.0, account);
                assert_eq!(
                    content_zoom(mode, rect.size.to_logical::<f64>(1.0).width),
                    1.0
                );
            }
        }
    }
    #[test]
    fn minimized_and_zero_client_layouts_preserve_warm_geometry_and_zoom() {
        for mode in [Mode::Personal, Mode::Work, Mode::Split] {
            for account in ACCOUNTS {
                let warm = desired_view_layout(mode, 1440.0, 750.0, 1.25, false, false, account);
                let mut applied = warm;
                for (minimized, width, height) in [
                    (true, 1440.0, 750.0),
                    (true, 0.0, 0.0),
                    (false, 0.0, 750.0),
                    (false, 1440.0, 0.0),
                ] {
                    let desired =
                        desired_view_layout(mode, width, height, 1.25, minimized, false, account);
                    let changes = desired.changes(applied);
                    assert_eq!(changes, ViewLayout::default());
                    applied.remember(changes);
                    assert_eq!(applied, warm);
                }
                assert_eq!(warm.changes(applied), ViewLayout::default());
            }
        }
    }
    #[test]
    fn dpi_change_refreshes_native_bounds_without_changing_zoom_or_visibility() {
        let mut applied =
            desired_view_layout(Mode::Split, 1440.0, 750.0, 1.0, false, false, "work");
        let desired = desired_view_layout(Mode::Split, 1440.0, 750.0, 1.25, false, false, "work");
        let changes = desired.changes(applied);
        assert_eq!(changes.geometry.unwrap().scale, 1.25);
        assert_eq!(changes.zoom, None);
        assert_eq!(changes.visible, None);
        applied.remember(changes);
        assert_eq!(desired.changes(applied), ViewLayout::default());
    }
    #[test]
    fn account_and_overlay_transitions_request_only_changed_view_properties() {
        for account in ACCOUNTS {
            let mut applied =
                desired_view_layout(Mode::Personal, 1440.0, 750.0, 1.25, false, false, account);
            let work = desired_view_layout(Mode::Work, 1440.0, 750.0, 1.25, false, false, account);
            let changes = work.changes(applied);
            assert_eq!(changes.geometry, None);
            assert_eq!(changes.zoom, None);
            assert_eq!(changes.visible, Some(account == "work"));
            applied.remember(changes);
            let split =
                desired_view_layout(Mode::Split, 1440.0, 750.0, 1.25, false, false, account);
            let changes = split.changes(applied);
            assert!(changes.geometry.is_some());
            assert!(changes.zoom.is_some_and(|zoom| zoom < 1.0));
            applied.remember(changes);
            let overlay =
                desired_view_layout(Mode::Split, 1440.0, 750.0, 1.25, false, true, account);
            let changes = overlay.changes(applied);
            assert_eq!(changes.geometry, None);
            assert_eq!(changes.zoom, None);
            assert_eq!(changes.visible, Some(false));
            applied.remember(changes);
            let changes = split.changes(applied);
            assert_eq!(changes.geometry, None);
            assert_eq!(changes.zoom, None);
            assert_eq!(changes.visible, Some(true));
            applied.remember(changes);
            assert_eq!(split.changes(applied), ViewLayout::default());
        }
    }
    #[test]
    fn minimized_account_switch_selects_visibility_then_restores_both_views() {
        for account in ACCOUNTS {
            let warm = desired_view_layout(Mode::Split, 1440.0, 750.0, 1.25, false, false, account);
            let mut applied = warm;
            let minimized = desired_view_layout(Mode::Work, 0.0, 0.0, 1.25, true, false, account);
            applied.remember(minimized.changes(applied));
            assert_eq!(applied.geometry, warm.geometry);
            assert_eq!(applied.zoom, warm.zoom);
            assert_eq!(applied.visible, Some(account == "work"));
            let restored =
                desired_view_layout(Mode::Work, 1440.0, 750.0, 1.25, false, false, account);
            let changes = restored.changes(applied);
            assert!(changes.geometry.is_some());
            assert_eq!(changes.zoom, Some(1.0));
            assert_eq!(changes.visible, None);
            applied.remember(changes);
            assert_eq!(applied, restored);
        }
    }
    #[test]
    fn external_navigation_rejects_executable_and_local_schemes() {
        for url in [
            "file:///C:/Windows/notepad.exe",
            "javascript:alert(1)",
            "powershell:test",
            "data:text/html,test",
        ] {
            assert!(!external_allowed(&url.parse().unwrap()));
        }
        assert!(external_allowed(&"https://example.com".parse().unwrap()));
    }
    #[test]
    fn download_paths_cannot_escape_download_directory_or_overwrite() {
        let dir = std::env::temp_dir().join(format!("switchboard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), "existing").unwrap();
        let mut reserved = HashSet::new();
        assert_eq!(
            unique_download(&dir, Path::new("../../report.pdf"), &mut reserved),
            dir.join("report (1).pdf")
        );
        assert_eq!(
            unique_download(&dir, Path::new("report.pdf"), &mut reserved),
            dir.join("report (2).pdf")
        );
        std::fs::remove_file(dir.join("report.pdf")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
