# Contributing

Bug reports, focused fixes, and documentation improvements are welcome.

## Development

Follow the Windows setup in [README.md](README.md), then run:

```powershell
npm ci
npm run dev
```

The interface is plain HTML/CSS/JavaScript in `ui/`. Rust code in `src-tauri/src/` owns the native workspace, isolated WebView2 sessions, shortcuts, preferences, tray, permissions, and notifications.

Before opening a pull request, run the checks in [TESTING.md](TESTING.md). Add focused tests for behaviour changes, and state which native Windows scenarios you actually exercised. Keep changes small enough to review.

## Design boundaries

- Preserve separate account profiles and explicit browser-permission consent.
- Keep remote WhatsApp pages outside the local shell's Tauri command permissions.
- Keep WebView2/COM objects on their owning thread and avoid synchronous browser work inside activation callbacks.
- Preserve usable normal, split, compact, minimized, and restored layouts.
- Use synthetic data in tests, previews, logs, and screenshots. Never commit linked-session storage, credentials, personal contacts, messages, or local diagnostics.
- Report operating-system shortcut conflicts without overriding another application's registration.

For bugs, include a clear reproduction and relevant software/display versions, with private content removed. For security vulnerabilities, follow [SECURITY.md](SECURITY.md).

By contributing, you agree that your contributions are provided under the repository's [MIT license](LICENSE).
