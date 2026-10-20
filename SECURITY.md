# Security

## Reporting a vulnerability

Use the repository's [private vulnerability reporting](https://github.com/yazanbaker94/switchboard/security/advisories/new) when available. If private reporting is unavailable, open a public issue asking for a private reporting channel without including the vulnerability details. Do not post session files, credentials, QR codes, or private conversations.

Include the affected version, reproduction steps using synthetic data, expected impact, and a minimal example where possible. There is no guaranteed response or support schedule.

## Security boundaries

Switchboard hosts WhatsApp Web in two separate WebView2 profiles. Only the bundled local interface can invoke workspace commands; remote account pages receive no shell capability. Notification permission requires an explicit action for each account. Native activation validates the app identity and account route.

Browser profiles contain sensitive linked-session storage. Keep the app's local data directory private and revoke a linked device through WhatsApp if it is exposed. Notification previews and Windows notification history can contain sender and message text according to the account and operating-system settings.

Local diagnostics may contain account counts, permission states, and filesystem paths. Review and redact them before sharing. Exclude browser profiles, build output, private screenshots, and machine-specific diagnostic files from public reports and pull requests.

The project depends on WhatsApp Web, WebView2, Tauri, Rust crates, and npm tooling. Keep the runtime and dependencies current. This project has not undergone an independent security audit; separate browser profiles are not a promise of protection against a compromised Windows account or device.
