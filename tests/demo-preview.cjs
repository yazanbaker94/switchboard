/*
 * Isolated, deterministic screenshot fixture. It reads only the checked-in UI
 * sources and serves original fictional conversations. It never reads profiles,
 * browser storage, diagnostics, messages, or account credentials.
 *
 * Run: npm run demo -- --port 4173
 * Views: /?view=split (default), /?view=compact, /?view=shortcuts
 * Capture at 1600 x 1000 after html[data-demo-ready="true"] appears.
 */
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const uiRoot = path.join(__dirname, '..', 'ui');

function installDemoBridge() {
  'use strict';
  const view = new URLSearchParams(location.search).get('view');
  const requestedView = ['split', 'compact', 'shortcuts'].includes(view) ? view : 'split';
  const labels = { personal: { name: 'Personal', label: 'Friends & family' }, work: { name: 'Work', label: 'Studio projects' } };
  const memoryStorage = new Map([['switchboard.labels.v1', JSON.stringify(labels)]]);
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem(key) { return memoryStorage.get(key) ?? null; },
    setItem(key, value) { memoryStorage.set(key, String(value)); },
    removeItem(key) { memoryStorage.delete(key); },
    clear() { memoryStorage.clear(); },
  } });
  const listeners = new Map();
  const binding = (accelerator) => ({ requestedAccelerator: accelerator, accelerator, registered: true, error: null, additionalAccelerators: [] });
  const state = {
    mode: requestedView === 'shortcuts' ? 'work' : 'split', lastSingleAccount: 'work',
    compactSidebar: requestedView === 'compact', overlay: false,
    loaded: { personal: true, work: true }, unread: { personal: 3, work: 7 },
    shortcuts: { summon: binding('Ctrl+Alt+W'), nextUnread: binding('Ctrl+Alt+U') },
  };
  const emit = (name, payload) => { for (const listener of listeners.get(name) || []) listener({ payload }); };
  window.__TAURI__ = {
    event: { async listen(name, listener) {
      const set = listeners.get(name) || new Set();
      set.add(listener); listeners.set(name, set);
      return () => set.delete(listener);
    } },
    core: { async invoke(command, args = {}) {
      switch (command) {
        case 'set_labels': return;
        case 'get_diagnostics': return structuredClone(state);
        case 'set_layout':
          state.mode = args.mode;
          if (args.mode !== 'split') state.lastSingleAccount = args.mode;
          emit('workspace-mode', state.mode); draw(); return;
        case 'set_compact_sidebar':
          state.compactSidebar = args.compact;
          emit('workspace-compact', args.compact); draw(); return;
        case 'set_overlay': state.overlay = args.open; draw(); return;
        case 'reload_account': emit('account-load', { account: args.account, state: 'loaded' }); return;
        case 'notification_permissions': return { personal: 'default', work: 'default' };
        case 'enable_notifications': throw Error('Demo only: no browser permission is changed.');
        case 'configure_shortcuts':
          for (const role of ['summon', 'nextUnread']) state.shortcuts[role] = { ...binding(args[role]), registered: !!args[role] };
          return structuredClone(state.shortcuts);
        default: throw Error('This operation is unavailable in the illustrative demo.');
      }
    } },
  };

  const datasets = {
    personal: {
      title: 'Weekend plans', initial: 'W', subtitle: 'Fictional group · 4 members',
      rows: [
        ['W', 'Weekend plans', 'See you there!', '10:14', ''],
        ['B', 'Book club', 'Which chapter are we reading?', '09:42', '2'],
        ['M', 'Morgan', 'That sounds like a great idea.', 'Yesterday', '1'],
        ['L', 'Little moments', 'Photo', 'Yesterday', ''],
        ['R', 'Recipe notes', 'Saved the lemon cake recipe.', 'Monday', ''],
      ],
      messages: [
        ['in', 'Milo', 'Picnic on Saturday? The forecast looks lovely. 🌿', '10:10'],
        ['out', '', 'Sounds good. I’ll bring the snacks.', '10:11'],
        ['in', 'Robin', 'Perfect. Meet by the big oak at 11?', '10:12'],
        ['out', '', 'See you there!', '10:14'],
      ],
    },
    work: {
      title: 'Studio crew', initial: 'S', subtitle: 'Fictional group · 4 members',
      rows: [
        ['S', 'Studio crew', 'Great. I’ll update the handoff notes.', '10:24', ''],
        ['A', 'Project Atlas', 'The second cover is ready to review.', '10:18', '4'],
        ['M', 'Monday check-in', 'A quick update for the team.', '09:30', '3'],
        ['I', 'Ideas & inspiration', 'A nice reference for the new layout.', 'Yesterday', ''],
        ['N', 'Notes to self', 'Keep it clear. Keep it simple.', 'Monday', ''],
      ],
      messages: [
        ['in', 'Avery', 'The launch mockups are ready for review.', '10:20'],
        ['in', 'Avery', 'I left two cover options in the board.', '10:20'],
        ['out', '', 'Option B reads better at small sizes.', '10:22'],
        ['out', '', 'Let’s use that for the preview.', '10:22'],
        ['in', 'Quinn', 'Great. I’ll update the handoff notes.', '10:24'],
      ],
    },
  };
  const bubbleIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 11.5a8 8 0 0 1-12 7l-4 1 1-4a8 8 0 1 1 15-4Z"/></svg>';
  const searchIcon = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></svg>';
  const nativeViews = document.createElement('div');
  nativeViews.className = 'demo-native-views';
  nativeViews.setAttribute('aria-label', 'Illustrative panes with fictional conversations');
  for (const account of ['personal', 'work']) {
    const data = datasets[account];
    const pane = document.createElement('section');
    pane.className = `demo-view ${account}`;
    pane.dataset.demoAccount = account;
    pane.innerHTML = `<div class="demo-wa">
      <aside class="demo-chat-list">
        <div class="demo-list-heading"><div><span class="demo-mini-brand">${bubbleIcon}</span><strong>Chats</strong></div><span class="demo-icons">＋ &nbsp; ⋮</span></div>
        <div class="demo-search">${searchIcon}<span>Search fictional chats</span></div>
        <div class="demo-filters"><span class="selected">All</span><span>Unread</span><span>Groups</span></div>
        <div class="demo-rows">${data.rows.map(([initial, name, preview, time, count], index) => `<div class="demo-chat-row ${index === 0 ? 'active' : ''}"><span class="demo-avatar avatar-${index}">${initial}</span><div class="demo-row-copy"><div><strong>${name}</strong><time>${time}</time></div><p>${preview}</p></div>${count ? `<span class="demo-unread">${count}</span>` : ''}</div>`).join('')}</div>
        <p class="demo-list-footer">Illustrative chat content only</p>
      </aside>
      <section class="demo-conversation">
        <header class="demo-conversation-header"><span class="demo-avatar">${data.initial}</span><div><strong>${data.title}</strong><span>${data.subtitle}</span></div><span class="demo-header-icons">${searchIcon}<i>⋮</i></span></header>
        <div class="demo-messages"><span class="demo-day">TODAY · DEMO</span><p class="demo-privacy">${bubbleIcon}Fictional messages created for this preview.</p><div class="demo-message-stack">${data.messages.map(([side, author, message, time]) => `<article class="demo-message ${side}">${author ? `<strong>${author}</strong>` : ''}<p>${message}</p><footer>${time}${side === 'out' ? '<span>✓✓</span>' : ''}</footer></article>`).join('')}</div></div>
        <div class="demo-composer"><span>☺</span><span>＋</span><div>Message <em>(demo)</em></div><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="12" rx="3"/><path d="M6 10v2a6 6 0 0 0 12 0v-2M12 18v3M9 21h6"/></svg></div>
      </section>
    </div>`;
    nativeViews.appendChild(pane);
  }
  document.querySelector('.workspace').appendChild(nativeViews);
  function draw() {
    nativeViews.hidden = state.overlay;
    nativeViews.dataset.mode = state.mode;
    const width = nativeViews.clientWidth;
    for (const pane of nativeViews.children) {
      const selected = state.mode === 'split' || state.mode === pane.dataset.demoAccount;
      pane.hidden = !selected;
      const paneWidth = state.mode === 'split' ? Math.max(1, (width - 4) / 2) : width;
      const zoom = state.mode === 'split' ? Math.min(1, Math.max(0.4, paneWidth / 900)) : 1;
      pane.style.setProperty('--demo-zoom', String(zoom));
    }
  }
  new ResizeObserver(draw).observe(nativeViews);
  draw();
  window.__SWITCHBOARD_DEMO__ = { requestedView, state, draw };
}

function prepareDemoCapture() {
  'use strict';
  const demo = window.__SWITCHBOARD_DEMO__;
  async function prepare() {
    for (let attempt = 0; attempt < 120; attempt += 1) {
      if (document.body.dataset.mode === demo.state.mode && document.body.dataset.compact === String(demo.state.compactSidebar)) break;
      await new Promise(requestAnimationFrame);
    }
    if (demo.requestedView === 'shortcuts') document.getElementById('shortcuts-button').click();
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
    demo.draw();
    document.documentElement.dataset.demoReady = 'true';
  }
  prepare();
}

const demoCss = String.raw`
.demo-badge{position:fixed;left:50%;top:25px;transform:translateX(-50%);z-index:90;padding:7px 12px;border:1px solid #6e644c;background:#302c23;color:#e9d2a4;border-radius:6px;font:600 9px "Segoe UI",sans-serif;letter-spacing:1.3px;pointer-events:none}.demo-native-views{position:absolute;inset:0;display:flex;gap:4px;background:#14191c}.demo-view{height:100%;flex:1;min-width:0;position:relative;overflow:hidden;--demo-zoom:1;--demo-accent:#a997cf;--demo-bubble:#283c3d}.demo-view.work{--demo-accent:#d9b780;--demo-bubble:#294138}.demo-wa{display:flex;width:calc(100% / var(--demo-zoom));height:calc(100% / var(--demo-zoom));transform:scale(var(--demo-zoom));transform-origin:left top;background:#10191d;color:#e3e9eb;font:14px "Segoe UI",sans-serif}.demo-wa svg{fill:none;stroke:currentColor;stroke-width:1.6;stroke-linecap:round;stroke-linejoin:round}.demo-chat-list{width:290px;flex-shrink:0;display:flex;flex-direction:column;background:#111c21;border-right:1px solid #2b363b}.demo-list-heading{height:85px;padding:0 23px;display:flex;align-items:center;justify-content:space-between}.demo-list-heading>div{display:flex;gap:10px;align-items:center}.demo-list-heading strong{font-size:25px;letter-spacing:-.5px;font-weight:650}.demo-mini-brand{color:var(--demo-accent)}.demo-mini-brand svg{width:25px;height:25px}.demo-icons{font-size:23px;color:#8c9ca4;letter-spacing:4px}.demo-search{display:flex;gap:13px;align-items:center;margin:0 14px 16px;padding:12px 13px;background:#202e34;border-radius:8px;color:#8d9ea7;font-size:12px}.demo-search svg{width:16px;height:16px;flex-shrink:0}.demo-filters{display:flex;gap:8px;padding:0 15px 20px}.demo-filters span{padding:7px 12px;border:1px solid #304047;border-radius:16px;color:#96a6ad;font-size:11px}.demo-filters .selected{background:#293b3c;border-color:#45524e;color:#c5d8cd}.demo-chat-row{display:flex;align-items:center;gap:12px;position:relative;padding:18px 15px;border-bottom:1px solid #202c32;height:89px}.demo-chat-row.active{background:#27353b}.demo-avatar{display:grid;place-items:center;width:44px;height:44px;flex-shrink:0;border-radius:50%;background:#46515b;color:#d8dee3;font-size:19px;font-weight:500}.demo-chat-row .avatar-0{background:#4d465f;color:#e5daf6}.work .demo-chat-row .avatar-0{background:#5d5140;color:#f1dcb9}.demo-chat-row .avatar-1{background:#314c53;color:#b8d7dc}.demo-chat-row .avatar-2{background:#4b4650;color:#d8c5d6}.demo-chat-row .avatar-3{background:#3f5249;color:#c2dbc7}.demo-chat-row .avatar-4{background:#4d4c3d;color:#d9d6b6}.demo-row-copy{flex:1;min-width:0}.demo-row-copy>div{display:flex;gap:7px;align-items:center;justify-content:space-between}.demo-row-copy strong{font-size:14px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.demo-row-copy time{font-size:9px;color:#93a39f;flex-shrink:0}.demo-row-copy p{font-size:11px;color:#9aacb4;margin:8px 13px 0 0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.demo-unread{position:absolute;right:14px;bottom:19px;display:grid;place-items:center;height:17px;min-width:17px;padding:0 4px;border-radius:9px;background:#87aa97;color:#14231c;font-size:10px;font-weight:600}.demo-list-footer{margin:auto 15px 20px;padding-top:20px;text-align:center;font-size:10px;color:#60757e}.demo-conversation{flex:1;min-width:0;display:flex;flex-direction:column;background-color:#10191d;background-image:radial-gradient(#3b514330 .7px,transparent .7px),linear-gradient(130deg,#17232388,transparent 60%);background-size:23px 23px,auto}.demo-conversation-header{height:79px;flex-shrink:0;display:flex;align-items:center;gap:13px;padding:0 21px;background:#202c31;border-bottom:1px solid #29383e}.demo-conversation-header .demo-avatar{width:41px;height:41px;background:#4d465f;color:#e5daf6}.work .demo-conversation-header .demo-avatar{background:#5d5140;color:#f1dcb9}.demo-conversation-header>div{min-width:0}.demo-conversation-header strong{display:block;font-size:16px;font-weight:500}.demo-conversation-header>div>span{display:block;margin-top:5px;color:#9cabb1;font-size:11px}.demo-header-icons{display:flex;gap:21px;align-items:center;margin-left:auto;color:#a0b0b6}.demo-header-icons svg{width:20px;height:20px}.demo-header-icons i{font-size:24px;font-style:normal}.demo-messages{display:flex;flex:1;min-height:0;flex-direction:column;align-items:center;padding:28px 36px 34px;overflow:hidden}.demo-day{font-size:10px;letter-spacing:.4px;color:#a5b9c2;background:#25343b;padding:7px 11px;border-radius:6px}.demo-privacy{display:flex;gap:7px;align-items:center;margin:19px 0 28px;color:#8fa398;font-size:10px}.demo-privacy svg{width:14px;height:14px}.demo-message-stack{width:100%;display:flex;flex-direction:column;gap:13px;margin-top:30px}.demo-message{max-width:85%;min-width:135px;padding:10px 12px 7px;border-radius:8px;background:#243238;align-self:flex-start;box-shadow:0 1px 2px #0003}.demo-message.out{align-self:flex-end;background:var(--demo-bubble)}.demo-message>strong{display:block;margin-bottom:6px;font-weight:550;font-size:11px;color:var(--demo-accent)}.demo-message p{margin:0;font-size:14px;line-height:21px;letter-spacing:.03px}.demo-message footer{display:flex;justify-content:flex-end;gap:5px;margin-top:5px;font-size:9px;color:#9faeae}.demo-message footer span{color:#8cbfc8;letter-spacing:-3px;margin-right:3px}.demo-composer{height:83px;flex-shrink:0;display:flex;align-items:center;gap:17px;padding:12px 19px;background:#202c31;border-top:1px solid #2c3b40;color:#acbcc2}.demo-composer>span{font-size:25px}.demo-composer>div{flex:1;min-width:0;border-radius:8px;padding:15px 17px;color:#a5b5bd;background:#2b393f;font-size:13px}.demo-composer em{font-style:normal;color:#748b96;font-size:11px;margin-left:4px}.demo-composer svg{width:23px;height:23px;flex-shrink:0}html[data-demo-ready=true] .loading-pane{display:none!important}@media(prefers-reduced-motion:reduce){.demo-wa{animation:none}}`;

function renderDemoHtml() {
  return fs.readFileSync(path.join(uiRoot, 'index.html'), 'utf8')
    .replace('<title>Switchboard</title>', '<title>Switchboard · Illustrative demo</title>')
    .replace('<script src="app.js" defer></script>', '<script src="/demo-bridge.js" defer></script>\n    <script src="/app.js" defer></script>\n    <script src="/demo-controls.js" defer></script>')
    .replace('</head>', '  <link rel="stylesheet" href="/demo.css" />\n  </head>')
    .replace('</body>', '  <div class="demo-badge" role="note">DEMO · FICTIONAL CONVERSATIONS</div>\n  </body>');
}

function createDemoServer() {
  const routes = new Map([
    ['/', ['text/html; charset=utf-8', renderDemoHtml()]],
    ['/styles.css', ['text/css; charset=utf-8', fs.readFileSync(path.join(uiRoot, 'styles.css'))]],
    ['/app.js', ['text/javascript; charset=utf-8', fs.readFileSync(path.join(uiRoot, 'app.js'))]],
    ['/demo.css', ['text/css; charset=utf-8', demoCss]],
    ['/demo-bridge.js', ['text/javascript; charset=utf-8', `(${installDemoBridge.toString()})();`]],
    ['/demo-controls.js', ['text/javascript; charset=utf-8', `(${prepareDemoCapture.toString()})();`]],
  ]);
  return http.createServer((request, response) => {
    const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
    const asset = routes.get(pathname);
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('X-Content-Type-Options', 'nosniff');
    response.setHeader('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src data:; connect-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'");
    if (!asset || !['GET', 'HEAD'].includes(request.method)) {
      response.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
      response.end('Demo asset not found.');
      return;
    }
    response.writeHead(200, { 'Content-Type': asset[0] });
    response.end(request.method === 'HEAD' ? undefined : asset[1]);
  });
}

if (require.main === module) {
  const flag = process.argv.indexOf('--port');
  const port = flag === -1 ? 4173 : Number(process.argv[flag + 1]);
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw Error('Use --port with a number from 0 to 65535.');
  const server = createDemoServer();
  server.listen(port, '127.0.0.1', () => {
    const base = `http://127.0.0.1:${server.address().port}`;
    process.stdout.write(`Illustrative demo only; no real accounts or profiles.\n${base}/?view=split\n${base}/?view=compact\n${base}/?view=shortcuts\nCapture at 1600 x 1000 after html[data-demo-ready="true"].\n`);
  });
}

module.exports = { createDemoServer, renderDemoHtml };
