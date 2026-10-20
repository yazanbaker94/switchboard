# Visual assets

- `hero.png` is AI-generated brand artwork inspired by the project's supplied design reference. It is an illustration, not a screenshot.
- `split-view.jpg`, `compact-view.jpg`, and `shortcuts.jpg` are browser captures of the bundled Switchboard shell with a demo-only Tauri bridge and fictional messaging panes. They are not captures of signed-in accounts or native WebView2 rendering.
- All conversations and account counts shown in the demo are invented. No real chats, contacts, profile photos, QR codes, or browser profiles are used.
- Screenshots can be recreated with `npm run demo` and the `split`, `compact`, or `shortcuts` view parameter described in the main README.

The banner was made with the built-in image-generation tool. Its final prompt is recorded below for reproducibility; the reference image is not included in this repository.

```text
Use case: ads-marketing
Asset type: GitHub README hero banner for an open-source Windows application.
Input image 1 is a style and product reference only, not an edit target. It shows the actual Switchboard visual language: dark charcoal shell, purple Personal account, amber Work account, two separately framed messaging panes.
Primary request: Create a beautifully polished wide 16:9 hero graphic for Switchboard. Make a refined dark product editorial with restrained soft purple and amber lighting, very subtle depth, crisp premium typography, lots of breathing room. On the left, the exact title "Switchboard" and exact subtitle "Two accounts. One workspace." plus a small "Windows · Rust · Tauri" line. On the right, an elegant miniature illustrative desktop window showing two adjacent blank messaging panes, a purple P account and amber W account, and abstract message bubbles with horizontal lines instead of chat text. This is brand artwork, not a functional screenshot. Match the provided product's dark palette and small paired-chat logo motif. No browser chrome. No phones or human figures.
Text (verbatim): "Switchboard"; "Two accounts. One workspace."; "Windows · Rust · Tauri".
Privacy constraints: absolutely no personal names, usernames, phone numbers, email addresses, QR codes, real faces, conversations, filesystem paths or account data. Any interface is abstract, no realistic chat content, no contacts. Do not copy the contacts or conversations from the reference. No WhatsApp logo, no Microsoft logo, no watermarks. Avoid excessive neon, noisy gradients or tiny illegible UI text.
```
