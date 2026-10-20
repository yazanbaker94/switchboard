//! Native desktop toast activation. COM objects stay in their MTA; only owned
//! activation arguments and the thread-safe app handle reach the UI bridge.

#[cfg(windows)]
mod platform {
    use std::{
        ffi::c_void,
        panic::{catch_unwind, AssertUnwindSafe},
        sync::{
            atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
            mpsc::{self, Receiver, Sender},
            Mutex,
        },
        thread::JoinHandle,
        time::Duration,
    };
    use tauri::AppHandle;
    use windows::{
        core::{implement, Error, IUnknown, Interface, Ref, Result, BOOL, GUID, PCWSTR},
        Win32::{
            Foundation::{CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER, E_UNEXPECTED},
            System::Com::{
                CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize,
                IClassFactory, IClassFactory_Impl, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
                REGCLS_MULTIPLEUSE,
            },
            UI::Notifications::{
                INotificationActivationCallback, INotificationActivationCallback_Impl,
                NOTIFICATION_USER_INPUT_DATA,
            },
        },
    };

    const MAX_AUMID_UNITS: usize = 128;
    const MAX_ROUTE_UNITS: usize = 512;

    static REGISTERED: AtomicBool = AtomicBool::new(false);
    static CALLBACKS: AtomicU64 = AtomicU64::new(0);
    static ACCEPTED: AtomicU64 = AtomicU64::new(0);
    static COMPLETED: AtomicU64 = AtomicU64::new(0);
    static REJECTED: AtomicU64 = AtomicU64::new(0);
    static FAILED: AtomicU64 = AtomicU64::new(0);
    static SERVER_LOCKS: AtomicU64 = AtomicU64::new(0);
    static LAST_HRESULT: AtomicI32 = AtomicI32::new(0);
    static SERVER: Mutex<Option<ServerControl>> = Mutex::new(None);

    struct ServerControl {
        stop: Sender<()>,
        thread: JoinHandle<()>,
    }

    struct ComApartment;

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    /// COM supplies valid marshalled strings. Bound every read, including the
    /// terminator, and reject malformed UTF-16 instead of changing its contents.
    unsafe fn bounded_string(value: PCWSTR, max_units: usize) -> Result<String> {
        if value.is_null() {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        for length in 0..max_units {
            if unsafe { *value.0.add(length) } == 0 {
                let units = unsafe { std::slice::from_raw_parts(value.0, length) };
                return String::from_utf16(units).map_err(|_| Error::from_hresult(E_INVALIDARG));
            }
        }
        Err(Error::from_hresult(E_INVALIDARG))
    }

    unsafe fn validated_route(aumid: PCWSTR, arguments: PCWSTR) -> Result<String> {
        if unsafe { bounded_string(aumid, MAX_AUMID_UNITS)? } != crate::notifications::AUMID {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let route = unsafe { bounded_string(arguments, MAX_ROUTE_UNITS)? };
        if !crate::notifications::is_activation_route(&route) {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        Ok(route)
    }

    #[implement(INotificationActivationCallback)]
    struct ToastCallback {
        app: AppHandle,
    }

    #[allow(non_snake_case)]
    impl INotificationActivationCallback_Impl for ToastCallback_Impl {
        fn Activate(
            &self,
            appusermodelid: &PCWSTR,
            invokedargs: &PCWSTR,
            _data: *const NOTIFICATION_USER_INPUT_DATA,
            _count: u32,
        ) -> Result<()> {
            CALLBACKS.fetch_add(1, Ordering::Relaxed);
            // Neither user-input pointer nor count is inspected or copied.
            let result = catch_unwind(AssertUnwindSafe(|| {
                let route = match unsafe { validated_route(*appusermodelid, *invokedargs) } {
                    Ok(route) => route,
                    Err(error) => {
                        REJECTED.fetch_add(1, Ordering::Relaxed);
                        return Err(error);
                    }
                };
                ACCEPTED.fetch_add(1, Ordering::Relaxed);
                // The bridge queues UI work and waits for a bounded acknowledgement.
                // No COM interface or callback-supplied pointer crosses that boundary.
                crate::notifications::activate_from_com(self.app.clone(), &route)
                    .map_err(|_| Error::from_hresult(E_FAIL))?;
                COMPLETED.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }))
            .unwrap_or_else(|_| Err(Error::from_hresult(E_UNEXPECTED)));
            if let Err(error) = &result {
                LAST_HRESULT.store(error.code().0, Ordering::Relaxed);
                if error.code() != E_INVALIDARG {
                    FAILED.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                LAST_HRESULT.store(0, Ordering::Relaxed);
            }
            result
        }
    }

    #[implement(IClassFactory)]
    struct ToastFactory {
        app: AppHandle,
    }

    #[allow(non_snake_case)]
    impl IClassFactory_Impl for ToastFactory_Impl {
        fn CreateInstance(
            &self,
            punkouter: Ref<'_, IUnknown>,
            riid: *const GUID,
            ppvobject: *mut *mut c_void,
        ) -> Result<()> {
            if ppvobject.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            unsafe { *ppvobject = std::ptr::null_mut() };
            if riid.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            if !punkouter.is_null() {
                return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
            }
            let callback: INotificationActivationCallback = ToastCallback {
                app: self.app.clone(),
            }
            .into();
            // QueryInterface supplies the caller's reference; the local reference
            // is released normally after this method returns.
            unsafe { callback.query(riid, ppvobject).ok() }
        }

        fn LockServer(&self, flock: BOOL) -> Result<()> {
            if flock.as_bool() {
                SERVER_LOCKS.fetch_add(1, Ordering::Relaxed);
            } else {
                let _ = SERVER_LOCKS.try_update(Ordering::Relaxed, Ordering::Relaxed, |locks| {
                    Some(locks.saturating_sub(1))
                });
            }
            // The dedicated server remains alive until explicit application exit,
            // regardless of the number of clients or outstanding server locks.
            Ok(())
        }
    }

    fn server_thread(
        app: AppHandle,
        ready: Sender<std::result::Result<(), i32>>,
        stop: Receiver<()>,
    ) {
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() } {
            LAST_HRESULT.store(error.code().0, Ordering::Relaxed);
            let _ = ready.send(Err(error.code().0));
            return;
        }
        let _apartment = ComApartment;
        let factory: IClassFactory = ToastFactory { app }.into();
        let cookie = match unsafe {
            CoRegisterClassObject(
                &crate::notifications::ACTIVATOR_CLSID,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
        } {
            Ok(cookie) => cookie,
            Err(error) => {
                LAST_HRESULT.store(error.code().0, Ordering::Relaxed);
                let _ = ready.send(Err(error.code().0));
                return;
            }
        };
        LAST_HRESULT.store(0, Ordering::Relaxed);
        REGISTERED.store(true, Ordering::Release);
        if ready.send(Ok(())).is_ok() {
            let _ = stop.recv();
        }
        if let Err(error) = unsafe { CoRevokeClassObject(cookie) } {
            LAST_HRESULT.store(error.code().0, Ordering::Relaxed);
        }
        REGISTERED.store(false, Ordering::Release);
        // Factory and apartment are released on the same initialized MTA thread.
        drop(factory);
    }

    pub fn start(app: &AppHandle) -> std::result::Result<(), String> {
        let mut server = SERVER
            .lock()
            .map_err(|_| "The toast activation server state is unavailable".to_string())?;
        if let Some(control) = server.as_ref() {
            if !control.thread.is_finished() {
                return if REGISTERED.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err("The toast activation server is still starting or stopping".into())
                };
            }
        }
        // A finished previous thread can be dropped without touching its COM state.
        let _ = server.take();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let app = app.clone();
        let thread = std::thread::Builder::new()
            .name("switchboard-toast-activation".into())
            .spawn(move || server_thread(app, ready_sender, stop_receiver))
            .map_err(|_| {
                LAST_HRESULT.store(E_FAIL.0, Ordering::Relaxed);
                "The toast activation server thread could not start".to_string()
            })?;
        *server = Some(ServerControl {
            stop: stop_sender,
            thread,
        });
        match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(code)) => Err(format!(
                "Toast activation registration failed (HRESULT {code:#010X})"
            )),
            Err(_) => {
                if let Some(control) = server.as_ref() {
                    let _ = control.stop.send(());
                }
                LAST_HRESULT.store(E_FAIL.0, Ordering::Relaxed);
                Err("The toast activation server did not report readiness".into())
            }
        }
    }

    pub fn stop() {
        if let Ok(mut server) = SERVER.lock() {
            if let Some(control) = server.take() {
                let _ = control.stop.send(());
                // Do not join the MTA from the UI thread: a concurrent callback
                // may still be awaiting its bounded UI acknowledgement.
            }
        }
    }

    pub fn status() -> serde_json::Value {
        serde_json::json!({
            "registered": REGISTERED.load(Ordering::Acquire),
            "callbacks": CALLBACKS.load(Ordering::Relaxed),
            "accepted": ACCEPTED.load(Ordering::Relaxed),
            "completed": COMPLETED.load(Ordering::Relaxed),
            "rejected": REJECTED.load(Ordering::Relaxed),
            "failed": FAILED.load(Ordering::Relaxed),
            "serverLocks": SERVER_LOCKS.load(Ordering::Relaxed),
            "lastHresult": LAST_HRESULT.load(Ordering::Relaxed),
        })
    }

    pub fn registered() -> bool {
        REGISTERED.load(Ordering::Acquire)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(Some(0)).collect()
        }

        #[test]
        fn activation_strings_are_bounded_and_utf16_is_not_replaced() {
            assert!(unsafe { bounded_string(PCWSTR::null(), 4) }.is_err());
            assert!(unsafe { bounded_string(PCWSTR([65u16; 4].as_ptr()), 4) }.is_err());
            assert!(unsafe { bounded_string(PCWSTR([0xd800u16, 0].as_ptr()), 2) }.is_err());
            let valid = [65u16, 66, 0];
            assert_eq!(
                unsafe { bounded_string(PCWSTR(valid.as_ptr()), 3) }.unwrap(),
                "AB"
            );
        }

        #[test]
        fn only_exact_app_identity_and_strict_activation_routes_are_accepted() {
            let aumid = wide(crate::notifications::AUMID);
            let route = wide("switchboard-wa://notification/work?id=7");
            assert!(
                unsafe { validated_route(PCWSTR(aumid.as_ptr()), PCWSTR(route.as_ptr())) }.is_ok()
            );
            let other = wide("com.switchboard.desktop.other");
            assert!(
                unsafe { validated_route(PCWSTR(other.as_ptr()), PCWSTR(route.as_ptr())) }.is_err()
            );
            let extra = wide("switchboard-wa://notification/work?id=7&extra=1");
            assert!(
                unsafe { validated_route(PCWSTR(aumid.as_ptr()), PCWSTR(extra.as_ptr())) }.is_err()
            );
        }
    }
}

#[cfg(windows)]
pub use platform::{registered, start, status, stop};

#[cfg(not(windows))]
pub fn start(_app: &tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn stop() {}

#[cfg(not(windows))]
pub fn registered() -> bool {
    false
}

#[cfg(not(windows))]
pub fn status() -> serde_json::Value {
    serde_json::json!({"registered": false})
}
