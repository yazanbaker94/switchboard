use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, MutexGuard, OnceLock,
};
use tauri::{plugin::TauriPlugin, AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub const DEFAULT_SUMMON: &str = "Ctrl+Alt+W";
pub const DEFAULT_NEXT_UNREAD: &str = "Ctrl+Alt+U";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Summon,
    NextUnread,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingStatus {
    pub requested_accelerator: String,
    pub accelerator: String,
    pub registered: bool,
    pub error: Option<String>,
    // Normally empty; if Windows refuses to unregister an owned key, report it
    // and keep its action working rather than silently leaving a live shortcut.
    pub additional_accelerators: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub summon: BindingStatus,
    pub next_unread: BindingStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Choice {
    accelerator: &'static str,
    shortcut: Shortcut,
}

#[derive(Clone)]
struct Binding {
    requested: String,
    owned: Vec<Choice>,
    error: Option<String>,
}

impl Binding {
    fn new(requested: &str) -> Self {
        Self {
            requested: requested.into(),
            owned: Vec::new(),
            error: None,
        }
    }

    fn status(&self) -> BindingStatus {
        let primary = self
            .owned
            .iter()
            .find(|choice| choice.accelerator == self.requested)
            .or_else(|| self.owned.first());
        BindingStatus {
            requested_accelerator: self.requested.clone(),
            accelerator: primary
                .map(|choice| choice.accelerator)
                .unwrap_or_default()
                .into(),
            registered: primary.is_some(),
            error: self.error.clone(),
            additional_accelerators: self
                .owned
                .iter()
                .filter(|choice| Some(*choice) != primary)
                .map(|choice| choice.accelerator.to_string())
                .collect(),
        }
    }
}

#[derive(Clone)]
struct State {
    summon: Binding,
    next_unread: Binding,
}

impl Default for State {
    fn default() -> Self {
        Self {
            summon: Binding::new(DEFAULT_SUMMON),
            next_unread: Binding::new(DEFAULT_NEXT_UNREAD),
        }
    }
}

impl State {
    fn status(&self) -> Status {
        Status {
            summon: self.summon.status(),
            next_unread: self.next_unread.status(),
        }
    }

    fn action(&self, shortcut: &Shortcut, phase: ShortcutState) -> Option<Action> {
        if phase != ShortcutState::Pressed {
            return None;
        }
        [
            (Action::Summon, &self.summon),
            (Action::NextUnread, &self.next_unread),
        ]
        .into_iter()
        .find_map(|(action, binding)| {
            binding
                .owned
                .iter()
                .any(|choice| choice.shortcut == *shortcut)
                .then_some(action)
        })
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static CONFIGURING: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Configuring;

impl Drop for Configuring {
    fn drop(&mut self) {
        CONFIGURING.store(false, Ordering::Release);
    }
}

fn parse_choice(action: Action, accelerator: &str) -> Result<Option<Choice>, String> {
    let accelerator = accelerator.trim();
    if accelerator.is_empty() || accelerator.eq_ignore_ascii_case("disabled") {
        return Ok(None);
    }
    let shortcut = accelerator
        .parse::<Shortcut>()
        .map_err(|_| "Choose one of the available global shortcuts".to_string())?;
    let allowed: &[(Code, &str)] = match action {
        Action::Summon => &[
            (Code::KeyW, DEFAULT_SUMMON),
            (Code::Space, "Ctrl+Alt+Space"),
        ],
        Action::NextUnread => &[
            (Code::KeyU, DEFAULT_NEXT_UNREAD),
            (Code::KeyN, "Ctrl+Alt+N"),
        ],
    };
    allowed
        .iter()
        .find(|(code, _)| shortcut.matches(Modifiers::CONTROL | Modifiers::ALT, *code))
        .map(|(_, accelerator)| {
            Some(Choice {
                accelerator,
                shortcut,
            })
        })
        .ok_or_else(|| "Choose one of the available global shortcuts for this action".into())
}

pub fn validate(summon: &str, next_unread: &str) -> Result<(), String> {
    parse_choice(Action::Summon, summon)?;
    parse_choice(Action::NextUnread, next_unread)?;
    Ok(())
}

trait Registration {
    fn register(&self, shortcut: Shortcut) -> Result<(), String>;
    fn unregister(&self, shortcut: Shortcut) -> Result<(), String>;
}

struct NativeRegistration<'a>(&'a AppHandle);

impl Registration for NativeRegistration<'_> {
    fn register(&self, shortcut: Shortcut) -> Result<(), String> {
        self.0
            .global_shortcut()
            .register(shortcut)
            .map_err(|error| error.to_string())
    }

    fn unregister(&self, shortcut: Shortcut) -> Result<(), String> {
        self.0
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|error| error.to_string())
    }
}

fn reconfigure(binding: &mut Binding, requested: Option<Choice>, native: &impl Registration) {
    binding.requested = requested
        .map(|choice| choice.accelerator)
        .unwrap_or_default()
        .into();
    binding.error = None;

    if let Some(choice) = requested {
        if !binding.owned.contains(&choice) {
            // Register first: an occupied replacement must not remove a working key.
            if let Err(error) = native.register(choice.shortcut) {
                eprintln!(
                    "Global shortcut {} registration: {error}",
                    choice.accelerator
                );
                binding.error = Some(format!(
                    "{} is unavailable. Choose another shortcut.",
                    choice.accelerator
                ));
                return;
            }
            binding.owned.push(choice);
        }
    }

    binding.owned.retain(|choice| {
        if Some(*choice) == requested {
            return true;
        }
        match native.unregister(choice.shortcut) {
            Ok(()) => false,
            Err(error) => {
                eprintln!("Global shortcut {} release: {error}", choice.accelerator);
                binding.error = Some(format!(
                    "Could not release {}. It remains active.",
                    choice.accelerator
                ));
                true
            }
        }
    });
}

/// Register each action independently. A conflict is reported in its status;
/// invalid settings or a concurrent configuration return an error without changes.
pub fn configure(app: &AppHandle, summon: &str, next_unread: &str) -> Result<Status, String> {
    let summon = parse_choice(Action::Summon, summon)?;
    let next_unread = parse_choice(Action::NextUnread, next_unread)?;
    CONFIGURING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| "Global shortcuts are already being changed. Try again.".to_string())?;
    let _configuring = Configuring;
    let mut next = state().clone();
    let native = NativeRegistration(app);

    // The plugin dispatches native operations to the main thread and can block.
    // Keep our state lock released, including while another hotkey is delivered.
    reconfigure(&mut next.summon, summon, &native);
    reconfigure(&mut next.next_unread, next_unread, &native);
    let result = next.status();
    *state() = next;
    Ok(result)
}

pub fn status() -> Status {
    state().status()
}

pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            let action = state().action(shortcut, event.state);
            if let Some(action) = action {
                let app = app.clone();
                // Leave WM_HOTKEY before operating the workspace/WebView2. The
                // Windows plugin uses MOD_NOREPEAT; Released events do no work.
                tauri::async_runtime::spawn(async move {
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        crate::workspace::handle_global_shortcut(&handle, action);
                    });
                });
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeRegistration {
        calls: RefCell<Vec<(bool, Shortcut)>>,
        occupied: Vec<Shortcut>,
        release_fails: Vec<Shortcut>,
    }

    impl Registration for FakeRegistration {
        fn register(&self, shortcut: Shortcut) -> Result<(), String> {
            self.calls.borrow_mut().push((true, shortcut));
            if self.occupied.contains(&shortcut) {
                Err("Already registered".into())
            } else {
                Ok(())
            }
        }

        fn unregister(&self, shortcut: Shortcut) -> Result<(), String> {
            self.calls.borrow_mut().push((false, shortcut));
            if self.release_fails.contains(&shortcut) {
                Err("Release failed".into())
            } else {
                Ok(())
            }
        }
    }

    fn choice(action: Action, accelerator: &str) -> Choice {
        parse_choice(action, accelerator).unwrap().unwrap()
    }

    #[test]
    fn presets_reject_other_actions_and_unmodified_typing_keys() {
        assert!(validate("Ctrl+Alt+W", "Ctrl+Alt+U").is_ok());
        assert!(validate("control + alt + SPACE", "CTRL+ALT+N").is_ok());
        assert!(validate("", "Disabled").is_ok());
        for invalid in ["W", "Ctrl+W", "Ctrl+Alt+U", "Ctrl+Alt+W+U", "Win+W"] {
            assert!(parse_choice(Action::Summon, invalid).is_err());
        }
        assert!(parse_choice(Action::NextUnread, "Ctrl+Alt+W").is_err());
    }

    #[test]
    fn occupied_replacement_preserves_the_working_binding() {
        let old = choice(Action::Summon, DEFAULT_SUMMON);
        let replacement = choice(Action::Summon, "Ctrl+Alt+Space");
        let mut binding = Binding::new(DEFAULT_SUMMON);
        binding.owned.push(old);
        let native = FakeRegistration {
            occupied: vec![replacement.shortcut],
            ..Default::default()
        };
        reconfigure(&mut binding, Some(replacement), &native);
        let status = binding.status();
        assert!(status.registered);
        assert_eq!(status.accelerator, DEFAULT_SUMMON);
        assert_eq!(status.requested_accelerator, "Ctrl+Alt+Space");
        assert!(status.error.is_some());
        assert_eq!(*native.calls.borrow(), [(true, replacement.shortcut)]);
    }

    #[test]
    fn replacement_registers_before_releasing_old_binding() {
        let old = choice(Action::Summon, DEFAULT_SUMMON);
        let replacement = choice(Action::Summon, "Ctrl+Alt+Space");
        let mut binding = Binding::new(DEFAULT_SUMMON);
        binding.owned.push(old);
        let native = FakeRegistration::default();
        reconfigure(&mut binding, Some(replacement), &native);
        assert_eq!(binding.owned, [replacement]);
        assert_eq!(
            *native.calls.borrow(),
            [(true, replacement.shortcut), (false, old.shortcut)]
        );
    }

    #[test]
    fn one_conflict_does_not_prevent_the_other_action_registering() {
        let summon = choice(Action::Summon, DEFAULT_SUMMON);
        let unread = choice(Action::NextUnread, DEFAULT_NEXT_UNREAD);
        let native = FakeRegistration {
            occupied: vec![summon.shortcut],
            ..Default::default()
        };
        let mut state = State::default();
        reconfigure(&mut state.summon, Some(summon), &native);
        reconfigure(&mut state.next_unread, Some(unread), &native);
        assert!(!state.status().summon.registered);
        assert!(state.status().next_unread.registered);
        assert_eq!(
            state.action(&unread.shortcut, ShortcutState::Pressed),
            Some(Action::NextUnread)
        );
    }

    #[test]
    fn disabling_releases_only_the_binding_we_own() {
        let old = choice(Action::Summon, DEFAULT_SUMMON);
        let mut binding = Binding::new(DEFAULT_SUMMON);
        binding.owned.push(old);
        let native = FakeRegistration::default();
        reconfigure(&mut binding, None, &native);
        assert!(!binding.status().registered);
        assert!(binding.status().accelerator.is_empty());
        assert_eq!(*native.calls.borrow(), [(false, old.shortcut)]);
    }

    #[test]
    fn failed_release_remains_reported_and_dispatches_without_release_events() {
        let old = choice(Action::Summon, DEFAULT_SUMMON);
        let replacement = choice(Action::Summon, "Ctrl+Alt+Space");
        let mut state = State::default();
        state.summon.owned.push(old);
        let native = FakeRegistration {
            release_fails: vec![old.shortcut],
            ..Default::default()
        };
        reconfigure(&mut state.summon, Some(replacement), &native);
        let status = state.status().summon;
        assert_eq!(status.accelerator, "Ctrl+Alt+Space");
        assert_eq!(status.additional_accelerators, [DEFAULT_SUMMON]);
        assert!(status.error.is_some());
        assert_eq!(
            state.action(&old.shortcut, ShortcutState::Pressed),
            Some(Action::Summon)
        );
        assert_eq!(
            state.action(&replacement.shortcut, ShortcutState::Released),
            None
        );
        assert_eq!(
            state.action(
                &choice(Action::NextUnread, DEFAULT_NEXT_UNREAD).shortcut,
                ShortcutState::Pressed
            ),
            None
        );
    }
}
