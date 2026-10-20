//! Account-labelled Windows notifications. Website permissions stay with WebView2.
//!
//! WebView2 COM objects are apartment-bound, so they live only in the UI thread's
//! map. WinRT callbacks and protocol activations send numeric IDs back to that
//! thread; chat text is never written to logs or files.

pub const AUMID: &str = "com.switchboard.desktop";
#[cfg(windows)]
pub(crate) const ACTIVATOR_CLSID: windows::core::GUID =
    windows::core::GUID::from_u128(0xfa77bc6a_2b23_41d2_a383_88ce46ccaf91);

#[cfg(windows)]
mod platform {
    use super::{ACTIVATOR_CLSID, AUMID};
    use std::{
        cell::RefCell,
        collections::{hash_map::RandomState, HashMap},
        hash::{BuildHasher, Hasher},
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
            Arc,
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use tauri::{AppHandle, Webview};
    use webview2_com::{
        take_pwstr,
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Notification, ICoreWebView2NotificationReceivedEventArgs, ICoreWebView2_24,
        },
        NotificationCloseRequestedEventHandler, NotificationReceivedEventHandler,
    };
    use windows::{
        core::{IInspectable, Interface, GUID, HSTRING, PCWSTR, PWSTR},
        Data::Xml::Dom::XmlDocument,
        Foundation::TypedEventHandler,
        Win32::{
            Foundation::PROPERTYKEY,
            System::{
                Com::{
                    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
                    StructuredStorage::{
                        InitPropVariantFromCLSID, PropVariantClear, PropVariantToStringAlloc,
                        PROPVARIANT,
                    },
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
                },
                Registry::{
                    RegCloseKey, RegCreateKeyW, RegQueryValueExW, RegSetValueExW, HKEY,
                    HKEY_CURRENT_USER, REG_SZ, REG_VALUE_TYPE,
                },
                Variant::VT_LPWSTR,
            },
            UI::Shell::{
                IShellLinkW, PropertiesSystem::IPropertyStore, SHStrDupW,
                SetCurrentProcessExplicitAppUserModelID, ShellLink, SLGP_RAWPATH,
            },
        },
        UI::Notifications::{
            NotificationSetting, ToastDismissalReason, ToastDismissedEventArgs,
            ToastFailedEventArgs, ToastNotification, ToastNotificationManager, ToastNotifier,
        },
    };

    const SCHEME: &str = "switchboard-wa";
    const SCHEME_DESCRIPTION: &str = "URL:Switchboard account notification";
    const MAX_TRACKED: usize = 128;
    const APP_ID_PROPERTY: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };
    const TOAST_ACTIVATOR_PROPERTY: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 26,
    };
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    static RECEIVED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    static SHOWN: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    static CLICKED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    static LAST_ERROR: AtomicI32 = AtomicI32::new(0);
    static LAST_SETTING_ERROR: AtomicI32 = AtomicI32::new(0);
    static SYNTHETIC_SHOWN: AtomicU64 = AtomicU64::new(0);

    fn index(account: &str) -> usize {
        if account == "work" {
            1
        } else {
            0
        }
    }

    fn notifications_enabled_or_unknown(notifier: &ToastNotifier) -> bool {
        match notifier.Setting() {
            Ok(setting) => setting == NotificationSetting::Enabled,
            Err(error) => {
                // Our Windows probe found no Setting record before the first
                // successful Show. An unknown setting must not block that Show;
                // Windows still enforces its notification settings and policy.
                LAST_SETTING_ERROR.store(error.code().0, Ordering::Relaxed);
                true
            }
        }
    }

    fn record_failed(args: Option<&ToastFailedEventArgs>) {
        if let Some(args) = args {
            let code = match args.ErrorCode() {
                Ok(code) => code.0,
                Err(error) => error.code().0,
            };
            LAST_ERROR.store(code, Ordering::Relaxed);
        }
    }

    pub fn status() -> serde_json::Value {
        let (setting, factory_error, setting_error) =
            match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID)) {
                Ok(notifier) => match notifier.Setting() {
                    Ok(setting) => (Some(setting.0), None, None),
                    Err(error) => (None, None, Some(error.code().0)),
                },
                Err(error) => (None, Some(error.code().0), None),
            };
        let history = ToastNotificationManager::History()
            .and_then(|history| history.GetHistoryWithId(&HSTRING::from(AUMID)));
        let mut synthetic_history = Vec::new();
        let history_count = history
            .as_ref()
            .ok()
            .and_then(|history| history.Size().ok());
        if let Ok(history) = history {
            for i in 0..history_count.unwrap_or(0) {
                if let Ok(toast) = history.GetAt(i) {
                    let group = toast.Group().ok().map(|group| group.to_string());
                    if group.as_deref() != Some("tests") {
                        continue;
                    }
                    if let Ok(xml) = toast.Content().and_then(|content| content.GetXml()) {
                        let xml = xml.to_string();
                        if xml.contains("Switchboard test") {
                            synthetic_history.push(serde_json::json!({"group":group,"personalLabel":xml.contains("Personal — Switchboard test"),"workLabel":xml.contains("Work — Switchboard test"),"foregroundActivation":xml.contains("activationType=\"foreground\"")}));
                        }
                    }
                }
            }
        }
        serde_json::json!({"registered":REGISTERED.load(Ordering::Acquire),"activation":crate::toast_activation::status(),"setting":setting,"factoryError":factory_error,"settingError":setting_error,"lastSettingError":LAST_SETTING_ERROR.load(Ordering::Relaxed),"historyCount":history_count,"syntheticHistory":synthetic_history,"syntheticShown":SYNTHETIC_SHOWN.load(Ordering::Relaxed),"received":{"personal":RECEIVED[0].load(Ordering::Relaxed),"work":RECEIVED[1].load(Ordering::Relaxed)},"shown":{"personal":SHOWN[0].load(Ordering::Relaxed),"work":SHOWN[1].load(Ordering::Relaxed)},"clicked":{"personal":CLICKED[0].load(Ordering::Relaxed),"work":CLICKED[1].load(Ordering::Relaxed)},"lastError":LAST_ERROR.load(Ordering::Relaxed)})
    }

    fn seed_ids() {
        // Old Action Center items survive process exit. Give every run a fresh
        // ID range so an old item cannot target a new browser notification.
        let mut hash = RandomState::new().build_hasher();
        hash.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        hash.write_u32(std::process::id());
        let seed = (hash.finish() & (u64::MAX >> 1)).max(1);
        let _ = NEXT_ID.compare_exchange(0, seed, Ordering::AcqRel, Ordering::Acquire);
    }

    thread_local! {
        static ACTIVE: RefCell<HashMap<u64, ActiveNotification>> = RefCell::new(HashMap::new());
    }

    struct ActiveNotification {
        account: String,
        pending_click: bool,
        tag: String,
        browser: ICoreWebView2Notification,
        toast: ToastNotification,
        notifier: ToastNotifier,
        close_token: i64,
    }

    impl ActiveNotification {
        fn dispose(self, report_closed: bool) {
            // Detach before calls that may synchronously run website callbacks.
            unsafe {
                let _ = self.browser.remove_CloseRequested(self.close_token);
            }
            let _ = self.notifier.Hide(&self.toast);
            if let Ok(history) = ToastNotificationManager::History() {
                let _ = history.RemoveGroupedTagWithId(
                    &HSTRING::from(self.tag.as_str()),
                    &HSTRING::from(self.account.as_str()),
                    &HSTRING::from(AUMID),
                );
            }
            if report_closed {
                unsafe {
                    let _ = self.browser.ReportClosed();
                }
            }
        }
    }

    fn remove(id: u64, report_closed: bool) {
        let entry = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            // Revealing the account may make WhatsApp close its notification.
            // Keep that original browser object until Activated reports its click.
            if !report_closed && active.get(&id).is_some_and(|entry| entry.pending_click) {
                return None;
            }
            active.remove(&id)
        });
        if let Some(entry) = entry {
            entry.dispose(report_closed);
        }
    }

    fn schedule_remove(app: &AppHandle, id: u64, report_closed: bool) {
        let handle = app.clone();
        // Always leave WinRT / WebView2 event callbacks before manipulating views.
        tauri::async_runtime::spawn(async move {
            let _ = handle.run_on_main_thread(move || remove(id, report_closed));
        });
    }

    fn is_account(account: &str) -> bool {
        matches!(account, "personal" | "work")
    }

    fn parse_activation(value: &str) -> Option<(String, u64)> {
        let url = tauri::Url::parse(value).ok()?;
        if url.scheme() != SCHEME
            || url.host_str() != Some("notification")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let account = url.path().strip_prefix('/')?;
        if !is_account(account) {
            return None;
        }
        let number = url.query()?.strip_prefix("id=")?;
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let id = number.parse::<u64>().ok()?;
        if id == 0 || value != format!("{SCHEME}://notification/{account}?id={id}") {
            return None;
        }
        Some((account.to_owned(), id))
    }

    pub fn is_activation_route(value: &str) -> bool {
        parse_activation(value).is_some()
    }

    fn report_click(id: u64) {
        let entry = ACTIVE.with(|active| active.borrow_mut().remove(&id));
        if let Some(entry) = entry {
            CLICKED[index(&entry.account)].fetch_add(1, Ordering::Relaxed);
            unsafe {
                let _ = entry.browser.ReportClicked();
            }
            entry.dispose(true);
        }
    }

    fn schedule_click(app: &AppHandle, id: u64) {
        let app = app.clone();
        // WinRT raises Activated after the COM activation callback returns.
        // Leave that event too before calling WebView2 or notification history.
        tauri::async_runtime::spawn(async move {
            let _ = app.run_on_main_thread(move || report_click(id));
        });
    }

    /// The MTA activation callback waits here while the UI consumes its grant.
    /// Only owned metadata crosses threads; browser notification objects stay on UI.
    pub fn activate_from_com(app: AppHandle, value: &str) -> Result<(), String> {
        let (account, id) = parse_activation(value).ok_or("Invalid notification route")?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let active = Arc::new(AtomicBool::new(true));
        let ui_active = active.clone();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let result = if ui_active.load(Ordering::Acquire) {
                ACTIVE.with(|active| {
                    if let Some(entry) = active.borrow_mut().get_mut(&id) {
                        if entry.account == account {
                            entry.pending_click = true;
                        }
                    }
                });
                crate::workspace::activate_account(&handle, &account).and_then(|_| {
                    if ui_active.load(Ordering::Acquire) {
                        crate::workspace::request_foreground(&handle, "com-activation")
                    } else {
                        Err("Notification activation expired".into())
                    }
                })
            } else {
                Err("Notification activation expired".into())
            };
            if result.is_err() {
                ACTIVE.with(|active| {
                    if let Some(entry) = active.borrow_mut().get_mut(&id) {
                        if entry.account == account {
                            entry.pending_click = false;
                        }
                    }
                });
            }
            let _ = sender.send(result);
        })
        .map_err(|error| error.to_string())?;
        let result = receiver.recv_timeout(std::time::Duration::from_secs(5));
        active.store(false, Ordering::Release);
        result
            .map_err(|_| "The UI did not acknowledge notification activation in time".to_string())?
    }

    /// Accept only the app's narrow notification route, including after restart.
    pub fn activate(app: &AppHandle, value: &str) -> bool {
        let Some((account, id)) = parse_activation(value) else {
            return false;
        };
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let action_app = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let _ = crate::workspace::activate_account(&action_app, &account);
                let matches = ACTIVE.with(|active| {
                    active
                        .borrow()
                        .get(&id)
                        .is_some_and(|entry| entry.account == account)
                });
                if matches {
                    report_click(id);
                }
                let _ = crate::workspace::request_foreground(&action_app, "protocol-deferred");
            });
        });
        true
    }

    fn escaped(value: &str, limit: usize) -> String {
        let mut result = String::new();
        for character in value
            .chars()
            .filter(|character| {
                matches!(*character, '\t' | '\n' | '\r')
                    || (*character >= ' ' && !matches!(*character, '\u{fffe}' | '\u{ffff}'))
            })
            .take(limit)
        {
            match character {
                '&' => result.push_str("&amp;"),
                '<' => result.push_str("&lt;"),
                '>' => result.push_str("&gt;"),
                '"' => result.push_str("&quot;"),
                '\'' => result.push_str("&apos;"),
                _ => result.push(character),
            }
        }
        result
    }

    fn is_whatsapp_origin(origin: &str) -> bool {
        // WebView2 SenderOrigin uses a trailing slash; accept only the two
        // canonical representations of this exact HTTPS origin.
        matches!(
            origin,
            "https://web.whatsapp.com" | "https://web.whatsapp.com/"
        )
    }

    unsafe fn relay(
        app: &AppHandle,
        account: &str,
        args: &ICoreWebView2NotificationReceivedEventArgs,
    ) -> windows::core::Result<()> {
        if !REGISTERED.load(Ordering::Acquire) || !crate::toast_activation::registered() {
            return Ok(());
        }
        let mut origin = PWSTR::null();
        args.SenderOrigin(&mut origin)?;
        if !is_whatsapp_origin(&take_pwstr(origin)) {
            return Ok(());
        }
        let browser = args.Notification()?;
        let mut title = PWSTR::null();
        browser.Title(&mut title)?;
        let title = take_pwstr(title);
        let mut body = PWSTR::null();
        browser.Body(&mut body)?;
        let body = take_pwstr(body);
        let mut silent = windows::core::BOOL::default();
        browser.IsSilent(&mut silent)?;

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let tag = format!("{id:x}");
        let label = crate::workspace::account_label(app, account);
        let xml = format!(
            "<toast activationType=\"foreground\" launch=\"{SCHEME}://notification/{account}?id={id}\"><visual><binding template=\"ToastGeneric\"><text>{} — {}</text><text>{}</text></binding></visual>{}</toast>",
            escaped(&label, 40),
            escaped(&title, 120),
            escaped(&body, 600),
            if silent.as_bool() { "<audio silent=\"true\"/>" } else { "" },
        );
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(tag.as_str()))?;
        toast.SetGroup(&HSTRING::from(account))?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?;
        if !notifications_enabled_or_unknown(&notifier) {
            // Preserve the browser's own notification path when this AUMID is
            // explicitly disabled; do not silently consume the website event.
            return Ok(());
        }

        // COM performs account selection and foreground activation. This event
        // runs afterward solely to open WhatsApp's original conversation/cleanup.
        let click_app = app.clone();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, _| {
                schedule_click(&click_app, id);
                Ok(())
            },
        ))?;
        let dismissed_app = app.clone();
        toast.Dismissed(&TypedEventHandler::<
            ToastNotification,
            ToastDismissedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if args.Reason()? != ToastDismissalReason::TimedOut {
                    schedule_remove(&dismissed_app, id, true);
                }
            }
            Ok(())
        }))?;
        let failed_app = app.clone();
        toast.Failed(
            &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, args| {
                record_failed(args.as_ref());
                schedule_remove(&failed_app, id, true);
                Ok(())
            }),
        )?;

        let close_app = app.clone();
        let close_handler =
            NotificationCloseRequestedEventHandler::create(Box::new(move |_, _| {
                // This originated in website code, which already knows it closed.
                schedule_remove(&close_app, id, false);
                Ok(())
            }));
        let mut close_token = 0;
        browser.add_CloseRequested(&close_handler, &mut close_token)?;
        let evicted = ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let evicted = if active.len() >= MAX_TRACKED {
                active
                    .keys()
                    .min()
                    .copied()
                    .and_then(|old| active.remove(&old))
            } else {
                None
            };
            active.insert(
                id,
                ActiveNotification {
                    account: account.to_owned(),
                    pending_click: false,
                    tag,
                    browser: browser.clone(),
                    toast: toast.clone(),
                    notifier: notifier.clone(),
                    close_token,
                },
            );
            evicted
        });
        if let Some(evicted) = evicted {
            evicted.dispose(true);
        }
        if let Err(error) = notifier.Show(&toast) {
            remove(id, false);
            return Err(error);
        }
        if let Err(error) = args.SetHandled(true) {
            remove(id, false);
            return Err(error);
        }
        // Handled cannot be undone. If the browser goes away at this moment,
        // retain the working custom toast and its account activation route.
        let _ = browser.ReportShown();
        SHOWN[index(account)].fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn install(view: &Webview, app: &AppHandle, account: &str) -> tauri::Result<()> {
        if !is_account(account) {
            return Ok(());
        }
        let app = app.clone();
        let account = account.to_owned();
        view.with_webview(move |platform| unsafe {
            let Ok(core) = platform.controller().CoreWebView2() else {
                return;
            };
            let Ok(core) = core.cast::<ICoreWebView2_24>() else {
                return;
            };
            let handler = NotificationReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    // Failure leaves Handled unchanged, allowing WebView2's
                    // default notification UI to remain the fallback.
                    RECEIVED[index(&account)].fetch_add(1, Ordering::Relaxed);
                    if let Err(error) = relay(&app, &account, &args) {
                        LAST_ERROR.store(error.code().0, Ordering::Relaxed);
                    }
                }
                Ok(())
            }));
            let mut token = 0;
            let _ = core.add_NotificationReceived(&handler, &mut token);
        })
    }

    /// Developer-only synthetic notification. It never reads website content or
    /// changes website permissions and uses the same protocol activation route.
    pub fn test_toast(app: &AppHandle, account: &str) -> Result<(), String> {
        if !is_account(account) {
            return Err("Unknown account for notification test".into());
        }
        if !REGISTERED.load(Ordering::Acquire) || !crate::toast_activation::registered() {
            return Err("Windows notification registration is unavailable".into());
        }
        let label = crate::workspace::account_label(app, account);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let route = format!("{SCHEME}://notification/{account}?id={id}");
        let xml = format!(
            "<toast activationType=\"foreground\" launch=\"{route}\"><visual><binding template=\"ToastGeneric\"><text>{} — Switchboard test</text><text>Desktop activation check. Click to open the {} account.</text></binding></visual></toast>",
            escaped(&label, 40), escaped(&label, 40),
        );
        let show = || -> windows::core::Result<()> {
            let document = XmlDocument::new()?;
            document.LoadXml(&HSTRING::from(xml))?;
            let toast = ToastNotification::CreateToastNotification(&document)?;
            toast.SetTag(&HSTRING::from(format!("test-{account}")))?;
            toast.SetGroup(&HSTRING::from("tests"))?;
            toast.Failed(
                &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
                    move |_, args| {
                        record_failed(args.as_ref());
                        Ok(())
                    },
                ),
            )?;
            let notifier =
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?;
            notifier.Show(&toast)
        };
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
            .map_err(|error| error.to_string())?;
        if !notifications_enabled_or_unknown(&notifier) {
            return Err("Windows notifications are disabled for Switchboard".into());
        }
        match show() {
            Ok(()) => {
                SYNTHETIC_SHOWN.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => {
                LAST_ERROR.store(error.code().0, Ordering::Relaxed);
                Err(error.to_string())
            }
        }
    }

    struct ComApartment;

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    unsafe fn shortcut_is_ours(path: &Path, executable: &Path) -> bool {
        let check = || -> windows::core::Result<bool> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let persist: IPersistFile = link.cast()?;
            let path = wide_path(path);
            persist.Load(PCWSTR(path.as_ptr()), STGM_READ)?;
            let mut owns_id = false;
            if let Ok(properties) = link.cast::<IPropertyStore>() {
                if let Ok(mut value) = properties.GetValue(&APP_ID_PROPERTY) {
                    owns_id = PropVariantToStringAlloc(&value)
                        .map(|value| take_pwstr(value) == AUMID)
                        .unwrap_or(false);
                    let _ = PropVariantClear(&mut value);
                }
            }
            let mut target = vec![0u16; 32768];
            link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
            let length = target
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(target.len());
            let target = String::from_utf16_lossy(&target[..length]);
            let current = executable.to_string_lossy();
            let same_program_name = Path::new(&target)
                .file_name()
                .zip(executable.file_name())
                .is_some_and(|(target, current)| {
                    target
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&current.to_string_lossy())
                });
            Ok(target.eq_ignore_ascii_case(&current) || (owns_id && same_program_name))
        };
        check().unwrap_or(false)
    }

    unsafe fn save_shortcut(path: &Path, executable: &Path) -> windows::core::Result<()> {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let executable_wide = wide_path(executable);
        link.SetPath(PCWSTR(executable_wide.as_ptr()))?;
        link.SetArguments(PCWSTR(wide("").as_ptr()))?;
        link.SetDescription(PCWSTR(
            wide("Two WhatsApp accounts in one clear workspace").as_ptr(),
        ))?;
        link.SetIconLocation(PCWSTR(executable_wide.as_ptr()), 0)?;
        if let Some(parent) = executable.parent() {
            let directory = wide_path(parent);
            link.SetWorkingDirectory(PCWSTR(directory.as_ptr()))?;
        }
        let properties: IPropertyStore = link.cast()?;
        let mut value = PROPVARIANT::default();
        let variant = &mut *value.Anonymous.Anonymous;
        variant.vt = VT_LPWSTR;
        variant.Anonymous.pwszVal = SHStrDupW(PCWSTR(wide(AUMID).as_ptr()))?;
        let set_result = properties.SetValue(&APP_ID_PROPERTY, &value);
        let _ = PropVariantClear(&mut value);
        set_result?;
        let mut activator = InitPropVariantFromCLSID(&ACTIVATOR_CLSID)?;
        let set_result = properties.SetValue(&TOAST_ACTIVATOR_PROPERTY, &activator);
        let _ = PropVariantClear(&mut activator);
        set_result?;
        properties.Commit()?;
        let persist: IPersistFile = link.cast()?;
        let path = wide_path(path);
        persist.Save(PCWSTR(path.as_ptr()), true)
    }

    fn ensure_start_menu_shortcut(executable: PathBuf) -> Result<(), String> {
        // Use a dedicated STA so registration does not depend on the caller's
        // existing COM apartment, and never pass shell COM objects across threads.
        std::thread::Builder::new()
            .name("switchboard-shortcut".into())
            .spawn(move || {
                unsafe {
                    CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                        .ok()
                        .map_err(|error| error.to_string())?;
                }
                let _apartment = ComApartment;
                let programs =
                    PathBuf::from(std::env::var_os("APPDATA").ok_or("APPDATA is unavailable")?)
                        .join(r"Microsoft\Windows\Start Menu\Programs");
                std::fs::create_dir_all(&programs).map_err(|error| error.to_string())?;
                for name in ["Switchboard.lnk", "Switchboard Workspace.lnk"] {
                    let path = programs.join(name);
                    let exists = path.try_exists().map_err(|error| error.to_string())?;
                    if exists && !unsafe { shortcut_is_ours(&path, &executable) } {
                        continue;
                    }
                    return unsafe { save_shortcut(&path, &executable) }
                        .map_err(|error| error.to_string());
                }
                Err("Existing Start Menu shortcuts belong to another application".into())
            })
            .map_err(|error| error.to_string())?
            .join()
            .map_err(|_| {
                "The Windows shortcut registration thread stopped unexpectedly".to_string()
            })?
    }

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    impl RegistryKey {
        fn create(path: &str) -> Result<Self, String> {
            let path = wide(path);
            let mut key = HKEY::default();
            unsafe {
                RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), &mut key)
                    .ok()
                    .map_err(|error| error.to_string())?;
            }
            Ok(Self(key))
        }

        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            let name = wide(name);
            let value = wide(value);
            let bytes: Vec<u8> = value
                .iter()
                .flat_map(|character| character.to_le_bytes())
                .collect();
            unsafe {
                RegSetValueExW(self.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes))
                    .ok()
                    .map_err(|error| error.to_string())
            }
        }

        fn get(&self, name: &str) -> Result<Option<String>, String> {
            let name = wide(name);
            let mut size = 0;
            let mut kind = REG_VALUE_TYPE::default();
            let status = unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut kind),
                    None,
                    Some(&mut size),
                )
            };
            if status.0 == 2 {
                return Ok(None);
            }
            status.ok().map_err(|error| error.to_string())?;
            if kind != REG_SZ {
                return Err("The notification URL scheme has an unexpected registration".into());
            }
            let mut buffer = vec![0u16; (size as usize + 1) / 2];
            unsafe {
                RegQueryValueExW(
                    self.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    Some(&mut kind),
                    Some(buffer.as_mut_ptr().cast()),
                    Some(&mut size),
                )
                .ok()
                .map_err(|error| error.to_string())?;
            }
            let length = buffer
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(buffer.len());
            Ok(Some(String::from_utf16_lossy(&buffer[..length])))
        }
    }

    /// Registration is explicit; merely importing or installing the hook does
    /// not change permissions, the registry, or the user's browser sessions.
    pub fn register(_app: &AppHandle) -> Result<(), String> {
        REGISTERED.store(false, Ordering::Release);
        let executable_path = std::env::current_exe().map_err(|error| error.to_string())?;
        let executable = executable_path
            .to_str()
            .ok_or("The app executable path is not valid Unicode")?;
        if executable.contains('"') {
            return Err("The app executable path cannot be registered".into());
        }
        let activator_text = format!("{{{ACTIVATOR_CLSID:?}}}");
        let server_path = format!(r"Software\Classes\CLSID\{activator_text}\LocalServer32");
        let server_command = format!("\"{executable}\" --toast-activated");
        let server = RegistryKey::create(&server_path)?;
        if server
            .get("")?
            .is_some_and(|command| !command.is_empty() && command != server_command)
        {
            return Err("The notification COM activator belongs to another installation".into());
        }
        let app_registration =
            RegistryKey::create(&format!(r"Software\Classes\AppUserModelId\{AUMID}"))?;
        if app_registration
            .get("CustomActivator")?
            .is_some_and(|owner| !owner.is_empty() && owner != activator_text)
        {
            return Err("The notification AUMID belongs to another activator".into());
        }
        let app_id = wide(AUMID);
        unsafe {
            SetCurrentProcessExplicitAppUserModelID(PCWSTR(app_id.as_ptr()))
                .map_err(|error| error.to_string())?;
        }
        let scheme = RegistryKey::create(&format!(r"Software\Classes\{SCHEME}"))?;
        if scheme
            .get("")?
            .is_some_and(|owner| !owner.is_empty() && owner != SCHEME_DESCRIPTION)
        {
            return Err(
                "The Switchboard notification URL scheme belongs to another application".into(),
            );
        }
        scheme.set("", SCHEME_DESCRIPTION)?;
        scheme.set("URL Protocol", "")?;
        RegistryKey::create(&format!(r"Software\Classes\{SCHEME}\shell\open\command"))?
            .set("", &format!("\"{executable}\" \"%1\""))?;
        app_registration.set("DisplayName", "Switchboard")?;
        app_registration.set("CustomActivator", &activator_text)?;
        server.set("", &server_command)?;
        ensure_start_menu_shortcut(executable_path)?;
        seed_ids();
        REGISTERED.store(true, Ordering::Release);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{escaped, is_whatsapp_origin, parse_activation};

        #[test]
        fn notification_origin_accepts_webview2_format_without_other_sites_or_paths() {
            assert!(is_whatsapp_origin("https://web.whatsapp.com/"));
            assert!(is_whatsapp_origin("https://web.whatsapp.com"));
            for origin in [
                "http://web.whatsapp.com/",
                "https://web.whatsapp.com.evil.test/",
                "https://web.whatsapp.com@evil.test/",
                "https://evil.test@web.whatsapp.com/",
                "https://web.whatsapp.com/path",
                "https://web.whatsapp.com/?x=1",
                "https://web.whatsapp.com/#x",
                "https://web.whatsapp.com//",
            ] {
                assert!(!is_whatsapp_origin(origin), "accepted {origin}");
            }
        }

        #[test]
        fn notification_routes_accept_only_the_two_accounts_and_numeric_ids() {
            assert_eq!(
                parse_activation("switchboard-wa://notification/personal?id=7"),
                Some(("personal".into(), 7))
            );
            assert_eq!(
                parse_activation("switchboard-wa://notification/work?id=42"),
                Some(("work".into(), 42))
            );
            for invalid in [
                "https://notification/work?id=1",
                "switchboard-wa://notification.evil/work?id=1",
                "switchboard-wa://user@notification/work?id=1",
                "switchboard-wa://notification:80/work?id=1",
                "switchboard-wa://notification/admin?id=1",
                "switchboard-wa://notification/%77ork?id=1",
                "switchboard-wa://notification/other/../work?id=1",
                "switchboard-wa://notification/work?id=1&command=open",
                "switchboard-wa://notification/work?id=1#extra",
                "switchboard-wa://notification/work?id=0",
                "switchboard-wa://notification/work?id=-1",
                "switchboard-wa://notification/work?id=01",
                "switchboard-wa://notification/work?id=18446744073709551616",
            ] {
                assert!(parse_activation(invalid).is_none());
            }
        }

        #[test]
        fn website_text_cannot_inject_toast_xml_or_invalid_control_characters() {
            assert_eq!(
                escaped("<text>&\"'\u{0}\u{1}\u{ffff}", 50),
                "&lt;text&gt;&amp;&quot;&apos;"
            );
            assert_eq!(escaped("مرحبا 👋", 5), "مرحبا");
        }
    }
}

#[cfg(windows)]
pub use platform::{
    activate, activate_from_com, install, is_activation_route, register, status, test_toast,
};

#[cfg(not(windows))]
pub fn install(
    _view: &tauri::Webview,
    _app: &tauri::AppHandle,
    _account: &str,
) -> tauri::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn register(_app: &tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn activate(_app: &tauri::AppHandle, _url: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn is_activation_route(_url: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn test_toast(_app: &tauri::AppHandle, _account: &str) -> Result<(), String> {
    Err("Synthetic Windows notifications are only available on Windows".into())
}
