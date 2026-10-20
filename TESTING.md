# Testing

## Automated checks

From the repository root on Windows with the development prerequisites installed:

```powershell
npm ci
npm run check
npm test -- --release
npm run test:ui
npm run test:demo
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

The latest recorded core validation passed **33 release Rust tests and 29 Node interface tests: 62 total**, with source, JavaScript syntax, and formatting checks passing. The browser demo has **2 additional passing tests** for its fixture, route allowlist, and isolation from private files and external connections.

Rust coverage includes account visibility, split/compact geometry and zoom, minimized/zero-size preservation, DPI invalidation, shortcut registration and conflict handling, preference writes and recovery, unread selection, tray rendering, download filename reservations, and notification activation/origin/XML validation.

Interface tests use mocked desktop APIs. They cover account-state synchronization, stale-result protection, compact settings overlays, custom labels, numeric unread updates, shortcut availability and recovery, and explicit notification consent. These tests do not establish live WhatsApp, native Windows notifications, or browser permission behaviour.

## Manually exercised on Windows

The following were observed in development builds using the installed WebView2 Runtime at **125% display scaling**:

- Two different accounts linked independently and remained authenticated after a full quit and restart.
- Mouse controls and `Ctrl+1/2/3` selected the corresponding single or split view.
- Normal, maximized, and restored layouts displayed full composers; split zoom returned to normal text size in single-account modes.
- Warm minimize/restore preserved formatted content. Tray hide/reopen retained the selected account without reloading. Layout counters showed no redundant native mutations in the tested unchanged-geometry restores; this is not a browser-paint benchmark.
- Labelled real notifications for both accounts selected the corresponding account and original conversation and brought the app forward. Explicit notification permission and WhatsApp's own notification setting were enabled.
- The global summon shortcut restored the app from minimized/hidden states, hid it while foreground, and brought it forward from another application.
- Next unread selected the account with the only positive reported count and closed the compact settings overlay.
- The compact rail, settings panel, and both split-view composers remained usable. A full quit with a panel open followed by restart restored split mode, last single account, compact choice, normal window size, and default shortcut registration without reopening the panel.

Notification delivery/click checks and the later workspace checks were separate runs. No new real-notification test was performed in the final workspace run. Core live checks used development builds; they are not a compatibility guarantee for every WhatsApp or WebView2 release.

## Coverage still needed

- Fresh installation and upgrade testing with the public application identity.
- Visual inspection of native tray dots, tooltip, and menu across both unread states. Native update status, count mapping, and tooltip-length checks passed.
- Actual OS shortcut conflicts, alternative-key saves, and disabling shortcuts; next unread with both/no positive counts and held-key behaviour. These have automated logic coverage.
- Maximized/minimized exit-and-restart persistence. Normal-size restart was observed; preservation rules have automated coverage.
- More monitor, scaling, resizing, and compact/expanded restore combinations.
- Long-idle restore, cold startup, and temporary network loss/recovery.
- Sending, reactions, groups, clipboard, uploads/downloads, drag-and-drop, microphone/camera permissions, voice notes, and calls.

## Reproducing a problem

Use accounts and sample content that you control. Record the app version, Windows version, WebView2 version, display scale, view mode, and reproduction steps. Compare a WhatsApp feature with the current WhatsApp Web browser version when useful.

Do not include real conversations, contact details, QR codes, browser profile folders, cookies, tokens, or unreviewed diagnostics in issues or test artifacts. Documentation artwork and interface previews use fictional demo content and are not proof of live WhatsApp behaviour. See [SECURITY.md](SECURITY.md) for vulnerability reporting.
