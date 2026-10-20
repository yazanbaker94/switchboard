//! Explicit, per-account notification consent from the trusted local shell.
//! Nothing in this module grants permissions at startup or on page load.

use tauri::{AppHandle, Webview};

const ACCOUNTS: [&str; 2] = ["personal", "work"];

fn validate_request(label: &str, account: &str) -> Result<(), String> {
    if label != "shell" {
        return Err("Only the local app shell can change notification permissions".into());
    }
    if !ACCOUNTS.contains(&account) {
        return Err("Unknown account".into());
    }
    Ok(())
}

/// Called only by the shell's explicit Allow notifications action. Keep this
/// command ACL-granted to the shell webview alone, like other workspace controls.
#[tauri::command]
pub async fn enable_notifications(
    view: Webview,
    app: AppHandle,
    account: String,
) -> Result<(), String> {
    validate_request(view.label(), &account)?;
    platform::grant(&app, &account).await?;
    platform::reload(&app, &account).await
}

/// Reads only the browser's notification permission enum, never page/chat data.
pub async fn states(app: AppHandle) -> Result<serde_json::Value, String> {
    let mut result = serde_json::Map::new();
    for account in ACCOUNTS {
        let permission = platform::state(&app, account).await?;
        result.insert(account.into(), serde_json::Value::String(permission));
    }
    Ok(serde_json::Value::Object(result))
}

fn parse_state(value: &str) -> Result<String, String> {
    let state: String = serde_json::from_str(value)
        .map_err(|_| "The browser notification permission is unavailable".to_string())?;
    if matches!(state.as_str(), "default" | "granted" | "denied") {
        Ok(state)
    } else {
        Err("The browser notification permission is unavailable".into())
    }
}

#[cfg(windows)]
mod platform {
    use super::parse_state;
    use std::sync::{Arc, Mutex};
    use tauri::{AppHandle, Manager};
    use tokio::sync::oneshot;
    use webview2_com::{
        ExecuteScriptCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        },
        SetPermissionStateCompletedHandler,
    };
    use windows::core::{Interface, PCWSTR};

    type Reply = Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>;

    fn complete(reply: &Reply, value: Result<String, String>) {
        let sender = reply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(value);
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    pub async fn grant(app: &AppHandle, account: &str) -> Result<(), String> {
        let account_view = app
            .get_webview(account)
            .ok_or("Account view is unavailable")?;
        let (sender, receiver) = oneshot::channel();
        let reply = Arc::new(Mutex::new(Some(sender)));
        account_view
            .with_webview(move |platform| unsafe {
                let operation = || -> windows::core::Result<()> {
                    let core = platform.controller().CoreWebView2()?;
                    let profile = core.cast::<ICoreWebView2_13>()?.Profile()?;
                    let profile = profile.cast::<ICoreWebView2Profile4>()?;
                    let callback_reply = reply.clone();
                    let completed =
                        SetPermissionStateCompletedHandler::create(Box::new(move |status| {
                            complete(
                                &callback_reply,
                                status
                                    .map(|_| "granted".into())
                                    .map_err(|error| error.to_string()),
                            );
                            Ok(())
                        }));
                    let origin = wide("https://web.whatsapp.com");
                    profile.SetPermissionState(
                        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                        PCWSTR(origin.as_ptr()),
                        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                        &completed,
                    )
                };
                if let Err(error) = operation() {
                    complete(&reply, Err(error.to_string()));
                }
            })
            .map_err(|error| error.to_string())?;
        receiver.await.map_err(|_| {
            "The account closed before the permission change finished".to_string()
        })??;
        Ok(())
    }

    pub async fn reload(app: &AppHandle, account: &str) -> Result<(), String> {
        let account_view = app
            .get_webview(account)
            .ok_or("Account view is unavailable")?;
        let (sender, receiver) = oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(account_view.reload().map_err(|error| error.to_string()));
        })
        .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "The account closed before reload finished".to_string())?
    }

    pub async fn state(app: &AppHandle, account: &str) -> Result<String, String> {
        let account_view = app
            .get_webview(account)
            .ok_or("Account view is unavailable")?;
        let (sender, receiver) = oneshot::channel();
        let reply = Arc::new(Mutex::new(Some(sender)));
        account_view
            .with_webview(move |platform| unsafe {
                let operation = || -> windows::core::Result<()> {
                    let core = platform.controller().CoreWebView2()?;
                    let callback_reply = reply.clone();
                    let completed =
                        ExecuteScriptCompletedHandler::create(Box::new(move |status, result| {
                            let result = status
                                .map_err(|error| error.to_string())
                                .and_then(|_| parse_state(&result));
                            complete(&callback_reply, result);
                            Ok(())
                        }));
                    let script = wide("Notification.permission");
                    core.ExecuteScript(PCWSTR(script.as_ptr()), &completed)
                };
                if let Err(error) = operation() {
                    complete(&reply, Err(error.to_string()));
                }
            })
            .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "The account closed before its permission could be read".to_string())?
    }
}

#[cfg(not(windows))]
mod platform {
    use tauri::AppHandle;

    pub async fn grant(_app: &AppHandle, _account: &str) -> Result<(), String> {
        Err("Notification permission management is only available on Windows".into())
    }

    pub async fn reload(_app: &AppHandle, _account: &str) -> Result<(), String> {
        Ok(())
    }

    pub async fn state(_app: &AppHandle, _account: &str) -> Result<String, String> {
        Err("Notification permission management is only available on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_state, validate_request};

    #[test]
    fn permission_changes_require_the_local_shell_and_a_known_account() {
        assert!(validate_request("shell", "personal").is_ok());
        assert!(validate_request("shell", "work").is_ok());
        for account in [
            "",
            "Personal",
            "work/../personal",
            "https://web.whatsapp.com",
            "other",
        ] {
            assert!(validate_request("shell", account).is_err());
        }
        for caller in ["personal", "work", "main", "shell-remote", ""] {
            assert!(validate_request(caller, "personal").is_err());
        }
    }

    #[test]
    fn permission_reads_return_only_the_three_browser_states() {
        for state in ["default", "granted", "denied"] {
            assert_eq!(parse_state(&format!("\"{state}\"")), Ok(state.into()));
        }
        for invalid in [
            "null",
            "true",
            "{}",
            "[]",
            "\"other\"",
            "\"granted\" trailing",
        ] {
            assert!(parse_state(invalid).is_err());
        }
    }
}
