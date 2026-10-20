const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '..', 'ui', 'app.js'), 'utf8');
const flush = () => new Promise(setImmediate);

// The shell has no frontend dependencies. This narrow DOM adapter exercises its
// event/IPC boundary without loading WhatsApp or accessing any account content.
async function boot(options = {}) {
  const nodes = new Map();
  const nativeEvents = new Map();
  const windowEvents = new Map();
  const documentEvents = new Map();
  const calls = [];
  const storageWrites = [];
  const pendingLayouts = [];
  const pendingOverlays = [];
  let resolveDiagnostics;
  let savedCompact = options.diagnostics?.compactSidebar ?? false;
  let nativeOverlay = false;
  const defaultShortcuts = {
    summon: { requestedAccelerator: 'Ctrl+Alt+W', accelerator: 'Ctrl+Alt+W', registered: true, error: null, additionalAccelerators: [] },
    nextUnread: { requestedAccelerator: 'Ctrl+Alt+U', accelerator: 'Ctrl+Alt+U', registered: true, error: null, additionalAccelerators: [] },
  };
  let nativeShortcuts = structuredClone(options.diagnostics?.shortcuts ?? defaultShortcuts);
  const browserPermissions = { personal: 'default', work: 'default', ...options.permissions };
  function element(key) {
    if (!nodes.has(key)) {
      const classes = new Map();
      nodes.set(key, {
        textContent: '', title: '', hidden: ['#settings-dialog', '#notifications-dialog', '#shortcuts-dialog'].includes(key), value: '', attrs: {}, events: {}, dataset: {},
        classList: { toggle(name, enabled) { classes.set(name, enabled); }, contains(name) { return !!classes.get(name); } },
        addEventListener(name, callback) { this.events[name] = callback; },
        setAttribute(name, value) { this.attrs[name] = value; },
        querySelector(selector) { return element(`${key} ${selector}`); },
        focus() {},
      });
    }
    return nodes.get(key);
  }
  const document = {
    title: '', body: { dataset: {} },
    querySelector: element,
    querySelectorAll(selector) {
      if (selector.startsWith('[data-name=')) return [element(`${selector} card`), element(`${selector} placeholder`)];
      return [];
    },
    addEventListener(name, callback) { documentEvents.set(name, callback); },
  };
  const context = {
    console: { error() {} }, document, structuredClone,
    setTimeout() { return 1; }, clearTimeout() {},
    localStorage: {
      getItem() { if (options.storageThrows) throw Error('unavailable'); return options.stored ?? null; },
      setItem(key, value) { storageWrites.push([key, value]); },
    },
    window: {
      innerWidth: 1440, innerHeight: 900,
      addEventListener(name, callback) { windowEvents.set(name, callback); },
      __TAURI__: {
        core: {
          invoke(command, args) {
            calls.push([command, args === undefined ? undefined : JSON.parse(JSON.stringify(args))]);
            if (command === 'set_labels' && options.labelsFail) return Promise.reject(Error('native label update failed'));
            if (command === 'set_layout' && options.pendingLayouts) return new Promise((resolve) => pendingLayouts.push(resolve));
            if (command === 'get_diagnostics') {
              if (options.pendingDiagnostics) return new Promise((resolve) => { resolveDiagnostics = resolve; });
              return Promise.resolve({
                mode: 'personal', lastSingleAccount: 'personal', compactSidebar: savedCompact,
                unread: { personal: null, work: null }, shortcuts: defaultShortcuts,
                loaded: { personal: false, work: false }, ...options.diagnostics,
              });
            }
            if (command === 'set_compact_sidebar') {
              if (options.compactFail) return Promise.reject(Error('compact preference unavailable'));
              savedCompact = args.compact;
            }
            if (command === 'set_overlay') {
              if (options.overlayFail) return Promise.reject(Error('overlay unavailable'));
              if (options.pendingOverlays) return new Promise((resolve) => {
                pendingOverlays.push({ open: args.open, complete() { nativeOverlay = args.open; resolve(); } });
              });
              nativeOverlay = args.open;
            }
            if (command === 'configure_shortcuts') {
              if (options.shortcutsFail) return Promise.reject(Error('shortcut preferences unavailable'));
              nativeShortcuts = Object.fromEntries(['summon', 'nextUnread'].map((role) => {
                const conflict = options.shortcutConflicts?.includes(args[role]);
                const previous = nativeShortcuts[role];
                return [role, {
                  requestedAccelerator: args[role],
                  accelerator: conflict ? previous.accelerator : args[role],
                  registered: conflict ? previous.registered : !!args[role],
                  error: conflict ? 'This shortcut is already in use.' : null,
                  additionalAccelerators: conflict ? previous.additionalAccelerators || [] : [],
                }];
              }));
              return Promise.resolve(nativeShortcuts);
            }
            if (command === 'notification_permissions') {
              if (options.permissionsFail) return Promise.reject(Error('permission query unavailable'));
              return Promise.resolve({ ...browserPermissions });
            }
            if (command === 'enable_notifications') {
              if (options.enableNotificationsFail) return Promise.reject(Error('permission update failed'));
              browserPermissions[args.account] = 'granted';
            }
            return Promise.resolve();
          },
        },
        event: { async listen(name, callback) { nativeEvents.set(name, callback); } },
      },
    },
  };
  vm.runInNewContext(source, context, { filename: 'ui/app.js' });
  await flush();
  return {
    element, document, calls, storageWrites, windowEvents, pendingLayouts, pendingOverlays,
    resolveDiagnostics(value) { resolveDiagnostics(value); },
    hasNativeEvent(name) { return nativeEvents.has(name); },
    savedCompact() { return savedCompact; },
    nativeOverlay() { return nativeOverlay; },
    emit(name, payload) { nativeEvents.get(name)({ payload }); },
    click(selector) { return element(selector).events.click(); },
    submit() { return element('#labels-form').events.submit({ preventDefault() {} }); },
    submitShortcuts() { return element('#shortcuts-form').events.submit({ preventDefault() {} }); },
    keydown(key) { return documentEvents.get('keydown')({ key }); },
    layoutCalls() { return calls.filter(([command]) => command === 'set_layout'); },
    domText() { return JSON.stringify({ title: document.title, nodes: [...nodes.values()].map(({ textContent, title, value, attrs }) => ({ textContent, title, value, attrs })) }); },
  };
}

test('native mode events synchronize header and controls without layout feedback', async () => {
  const app = await boot();
  assert.equal(app.layoutCalls().length, 0);
  app.emit('workspace-mode', 'work');
  assert.equal(app.document.body.dataset.mode, 'work');
  assert.equal(app.element('#active-name').textContent, 'Work');
  assert.equal(app.element('#account-personal').attrs['aria-pressed'], 'false');
  assert.equal(app.element('#account-work').attrs['aria-pressed'], 'true');
  assert.equal(app.element('#reload-button').attrs['aria-label'], 'Reload Work');
  app.emit('workspace-mode', 'split');
  assert.equal(app.element('#active-name').textContent, 'Both accounts');
  assert.equal(app.element('#split-button').attrs['aria-pressed'], 'true');
  assert.equal(app.element('#reload-button').attrs['aria-label'], 'Reload both accounts');
  assert.equal(app.layoutCalls().length, 0);
  app.emit('workspace-mode', 'invalid');
  assert.equal(app.document.body.dataset.mode, 'split');
});

test('rapid user switches invoke immediately and preserve order even with pending IPC', async () => {
  const app = await boot({ pendingLayouts: true });
  app.click('#account-work');
  app.click('#account-personal');
  app.click('#split-button');
  assert.deepEqual(app.layoutCalls().map(([, args]) => args.mode), ['work', 'personal', 'split']);
  assert.equal(app.pendingLayouts.length, 3, 'no frontend promise queue delays later mode requests');
  for (const resolve of app.pendingLayouts) resolve();
  await flush();
});

test('window resizing has no frontend IPC handler', async () => {
  const app = await boot();
  assert.equal(app.windowEvents.has('resize'), false);
  assert.equal(app.layoutCalls().length, 0);
});

test('invalid stored preferences safely fall back to valid defaults', async (t) => {
  for (const [name, options] of [
    ['malformed JSON', { stored: '{invalid' }],
    ['wrong field types', { stored: JSON.stringify({ personal: { name: 42, label: [] }, work: { name: false, label: {} } }) }],
    ['blank names', { stored: JSON.stringify({ personal: { name: '   ' }, work: null }) }],
    ['unavailable store', { storageThrows: true }],
  ]) {
    await t.test(name, async () => {
      const app = await boot(options);
      assert.equal(app.element('#active-name').textContent, 'Personal');
      assert.deepEqual(app.calls[0], ['set_labels', { personal: 'Personal', work: 'Work' }]);
      assert.equal(app.layoutCalls().length, 0);
    });
  }
});

test('only numeric native unread counts enter the DOM; title content is neither consumed, stored, nor forwarded', async () => {
  const app = await boot();
  const secret = 'PRIVATE_CHAT_TITLE_SENTINEL';
  assert.equal(app.hasNativeEvent('account-title'), false);
  app.emit('account-unread', { account: 'work', count: 27, title: secret });
  assert.equal(app.element('[data-unread="work"]').textContent, '27');
  assert.equal(app.element('[data-unread="work"]').title, '27 unread chats');
  assert.equal(app.element('[data-unread="work"]').hidden, false);
  assert.equal(app.domText().includes(secret), false);
  assert.equal(JSON.stringify(app.calls).includes(secret), false);
  assert.equal(JSON.stringify(app.storageWrites).includes(secret), false);
  app.emit('account-unread', { account: 'work', count: null });
  assert.equal(app.element('[data-unread="work"]').hidden, true);
  assert.equal(app.domText().includes(secret), false);
  app.emit('account-unread', { account: 'personal', count: 123 });
  assert.equal(app.element('[data-unread="personal"]').textContent, '99+');
  assert.equal(app.element('[data-unread="personal"]').title, '123 unread chats');
  app.emit('account-unread', { account: 'personal', count: -5 });
  assert.equal(app.element('[data-unread="personal"]').hidden, true);
});

test('native labels synchronize before startup diagnostics and after saving, excluding phone labels', async () => {
  const app = await boot({ stored: JSON.stringify({ personal: { name: 'Family', label: 'example primary line' }, work: { name: 'Studio', label: 'example office line' } }) });
  assert.deepEqual(app.calls.slice(0, 2).map(([command]) => command), ['set_labels', 'get_diagnostics']);
  assert.deepEqual(app.calls[0][1], { personal: 'Family', work: 'Studio' });
  app.element('#personal-name').value = 'Friends';
  app.element('#work-name').value = 'Office';
  app.element('#personal-label').value = 'private phone label';
  app.element('#work-label').value = 'another private phone label';
  await app.submit();
  assert.deepEqual(app.calls.at(-1), ['set_labels', { personal: 'Friends', work: 'Office' }]);
  assert.equal(app.element('#active-name').textContent, 'Friends');
  assert.equal(app.storageWrites.length, 1);
  assert.equal(JSON.stringify(app.calls).includes('private phone label'), false);
});

test('native label failure shows a retry message and still reads the native workspace', async () => {
  const app = await boot({ labelsFail: true });
  assert.equal(app.calls.some(([command]) => command === 'get_diagnostics'), true);
  assert.equal(app.element('#toast').hidden, false);
  assert.match(app.element('#toast').textContent, /Could not update tray labels/);
});

test('startup preserves native Work activation and recovers loaded state without changing layout', async () => {
  const app = await boot({ diagnostics: { mode: 'work', loaded: { personal: false, work: true } } });
  assert.equal(app.document.body.dataset.mode, 'work');
  assert.equal(app.element('#active-name').textContent, 'Work');
  assert.equal(app.element('#reload-button').attrs['aria-label'], 'Reload Work');
  assert.equal(app.element('#account-personal').classList.contains('is-loading'), true);
  assert.equal(app.element('#account-work').classList.contains('is-loading'), false);
  assert.equal(app.layoutCalls().length, 0, 'startup must never overwrite native geometry or mode');
});

test('newer native mode and loading events win over a delayed startup snapshot', async () => {
  const app = await boot({ pendingDiagnostics: true });
  app.emit('workspace-mode', 'work');
  app.emit('account-load', { account: 'work', state: 'loaded' });
  app.resolveDiagnostics({ mode: 'personal', loaded: { personal: false, work: false } });
  await flush();
  assert.equal(app.document.body.dataset.mode, 'work');
  assert.equal(app.element('#account-work').classList.contains('is-loading'), false);
  assert.equal(app.layoutCalls().length, 0);
});

test('loaded native accounts expose no stale loading placeholder', async () => {
  const app = await boot({ diagnostics: { mode: 'personal', loaded: { personal: true, work: false } } });
  assert.equal(app.element('[data-pane="personal"]').hidden, true);
  assert.equal(app.element('[data-pane="work"]').hidden, true, 'inactive loading account is hidden');
  app.emit('workspace-mode', 'work');
  assert.equal(app.element('[data-pane="work"]').hidden, false, 'selected account exposes confirmed loading copy');
  app.emit('account-load', { account: 'work', state: 'loaded' });
  assert.equal(app.element('[data-pane="work"]').hidden, true);
  app.emit('workspace-mode', 'split');
  assert.equal(app.element('[data-pane="personal"]').hidden, true);
  assert.equal(app.element('[data-pane="work"]').hidden, true);
});

test('desktop alerts startup and panel opening never grant permission', async () => {
  const app = await boot({ permissions: { personal: 'granted', work: 'denied' } });
  assert.equal(app.calls.some(([command]) => command === 'notification_permissions'), false, 'permission query is deferred until panel opens');
  assert.equal(app.calls.some(([command]) => command === 'enable_notifications'), false);
  await app.click('#notifications-button');
  assert.equal(app.element('#notifications-dialog').hidden, false);
  assert.equal(app.element('[data-notification-status="personal"]').textContent, 'Browser permission allowed');
  assert.equal(app.element('#allow-notifications-personal').disabled, true);
  assert.equal(app.element('[data-notification-status="work"]').textContent, 'Browser permission denied');
  assert.equal(app.element('#allow-notifications-work').disabled, false);
  assert.equal(app.calls.filter(([command]) => command === 'notification_permissions').length, 1);
  assert.equal(app.calls.some(([command]) => command === 'enable_notifications'), false, 'reading permission does not grant it');
});

test('only a manual allow button invokes notification consent for the chosen account', async () => {
  const app = await boot();
  await app.click('#notifications-button');
  assert.equal(app.element('[data-notification-status="work"]').textContent, 'Browser permission not allowed');
  await app.click('#allow-notifications-work');
  assert.deepEqual(app.calls.filter(([command]) => command === 'enable_notifications'), [['enable_notifications', { account: 'work' }]]);
  assert.equal(app.element('[data-notification-status="work"]').textContent, 'Browser permission allowed');
  assert.equal(app.element('#allow-notifications-work').textContent, 'Allowed ✓');
  assert.equal(app.element('#allow-notifications-work').disabled, true);
  assert.equal(app.element('[data-notification-status="personal"]').textContent, 'Browser permission not allowed');
  await app.click('#allow-notifications-work');
  assert.equal(app.calls.filter(([command]) => command === 'enable_notifications').length, 1, 'already granted accounts are not granted repeatedly');
});

test('notification consent failure remains ungranted and offers an honest retry', async () => {
  const app = await boot({ enableNotificationsFail: true });
  await app.click('#notifications-button');
  await app.click('#allow-notifications-personal');
  assert.equal(app.element('[data-notification-status="personal"]').textContent, 'Could not allow notifications. Try again.');
  assert.equal(app.element('#allow-notifications-personal').textContent, 'Try again');
  assert.equal(app.element('#allow-notifications-personal').disabled, false);
  assert.equal(app.element('#allow-notifications-personal').classList.contains('permission-granted'), false);
  assert.equal(app.element('[data-notification-status="work"]').textContent, 'Browser permission not allowed');
});

test('alerts and labels panels are exclusive, escape dismisses alerts, and custom names follow labels', async () => {
  const app = await boot();
  app.click('#settings-button');
  assert.equal(app.element('#settings-dialog').hidden, false);
  await app.click('#notifications-button');
  assert.equal(app.element('#settings-dialog').hidden, true);
  assert.equal(app.element('#settings-button').attrs['aria-expanded'], 'false');
  assert.equal(app.element('#notifications-dialog').hidden, false);
  app.keydown('Escape');
  assert.equal(app.element('#notifications-dialog').hidden, true);
  assert.equal(app.element('#notifications-button').attrs['aria-expanded'], 'false');
  app.element('#personal-name').value = 'Friends';
  app.element('#work-name').value = 'Studio';
  await app.submit();
  assert.equal(app.element('#allow-notifications-work').attrs['aria-label'], 'Allow desktop notifications for Studio');
  await app.click('#notifications-button');
  app.click('#settings-button');
  assert.equal(app.element('#notifications-dialog').hidden, true);
  assert.equal(app.element('#notifications-button').attrs['aria-expanded'], 'false');
  assert.equal(app.calls.some(([command]) => command === 'enable_notifications'), false);
});

test('native startup restores compact rail, unread counts, custom labels, and saved Split return account without writeback', async () => {
  const app = await boot({
    stored: JSON.stringify({ personal: { name: 'Family', label: 'example personal line' }, work: { name: 'Studio', label: 'example work line' } }),
    diagnostics: { mode: 'split', lastSingleAccount: 'work', compactSidebar: true, unread: { personal: 3, work: 12 } },
  });
  assert.equal(app.document.body.dataset.compact, 'true');
  assert.equal(app.element('#sidebar-toggle').attrs['aria-expanded'], 'false');
  assert.equal(app.element('#sidebar-toggle').attrs['aria-label'], 'Expand account sidebar');
  assert.equal(app.element('[data-unread="work"]').textContent, '12');
  assert.match(app.element('#account-work').attrs['aria-label'], /Studio, example work line/);
  assert.match(app.element('#account-work').title, /Studio · example work line/);
  assert.deepEqual(app.calls.map(([command]) => command), ['set_labels', 'get_diagnostics']);
  assert.equal(app.storageWrites.length, 0, 'existing labels are not rewritten on startup');
  app.click('#split-button');
  assert.equal(app.layoutCalls().at(-1)[1].mode, 'work');
});

test('compact toggle persists only explicit changes and native compact events have no invoke feedback', async () => {
  const app = await boot();
  await app.click('#sidebar-toggle');
  assert.deepEqual(app.calls.filter(([command]) => command === 'set_compact_sidebar'), [['set_compact_sidebar', { compact: true }]]);
  assert.equal(app.savedCompact(), true);
  assert.equal(app.document.body.dataset.compact, 'true');
  assert.equal(app.layoutCalls().length, 0, 'compact geometry is owned by the native compact command');
  const callsBefore = app.calls.length;
  app.emit('workspace-compact', false);
  assert.equal(app.document.body.dataset.compact, 'false');
  assert.equal(app.calls.length, callsBefore);
  app.emit('workspace-compact', 'invalid');
  assert.equal(app.document.body.dataset.compact, 'false');
});

test('newer compact, unread, and mode changes win over a delayed startup snapshot', async () => {
  const app = await boot({ pendingDiagnostics: true });
  await app.click('#sidebar-toggle');
  app.emit('workspace-mode', 'work');
  app.emit('account-unread', { account: 'work', count: 9 });
  app.resolveDiagnostics({ mode: 'personal', lastSingleAccount: 'personal', compactSidebar: false, unread: { personal: 2, work: 1 } });
  await flush();
  assert.equal(app.document.body.dataset.mode, 'work');
  assert.equal(app.document.body.dataset.compact, 'true');
  assert.equal(app.element('[data-unread="work"]').textContent, '9');
  assert.equal(app.element('[data-unread="personal"]').textContent, '2');
  assert.equal(app.layoutCalls().length, 0);
});

test('compact dialogs use native overlay and never overwrite saved compact preference, including while open at quit', async () => {
  const app = await boot({ diagnostics: { compactSidebar: true } });
  await app.click('#settings-button');
  assert.equal(app.element('#settings-dialog').hidden, false);
  assert.equal(app.nativeOverlay(), true);
  assert.equal(app.savedCompact(), true, 'quitting while the editor is open must retain compact=true');
  assert.equal(app.document.body.dataset.compact, 'true');
  await app.click('#notifications-button');
  assert.equal(app.element('#settings-dialog').hidden, true);
  assert.equal(app.element('#notifications-dialog').hidden, false);
  await app.click('#shortcuts-button');
  assert.equal(app.element('#notifications-dialog').hidden, true);
  assert.equal(app.element('#shortcuts-dialog').hidden, false);
  assert.deepEqual(app.calls.filter(([command]) => command === 'set_overlay'), [['set_overlay', { open: true }]], 'switching panels keeps one overlay active');
  app.keydown('Escape');
  await flush();
  assert.equal(app.element('#shortcuts-dialog').hidden, true);
  assert.equal(app.nativeOverlay(), false);
  assert.equal(app.savedCompact(), true);
  assert.equal(app.calls.some(([command]) => command === 'set_compact_sidebar'), false);
});

test('rail toggle closes a compact dialog and restores native views before changing preference', async () => {
  const app = await boot({ diagnostics: { compactSidebar: true } });
  await app.click('#settings-button');
  await app.click('#sidebar-toggle');
  assert.equal(app.element('#settings-dialog').hidden, true);
  assert.equal(app.nativeOverlay(), false);
  assert.equal(app.document.body.dataset.compact, 'false');
  assert.deepEqual(app.calls.filter(([command]) => ['set_overlay', 'set_compact_sidebar'].includes(command)), [
    ['set_overlay', { open: true }], ['set_overlay', { open: false }], ['set_compact_sidebar', { compact: false }],
  ]);
});

test('native account activation dismisses compact settings overlay without changing its saved preference', async () => {
  const app = await boot({ diagnostics: { compactSidebar: true } });
  await app.click('#shortcuts-button');
  assert.equal(app.nativeOverlay(), true);
  app.emit('workspace-mode', 'work');
  await flush();
  assert.equal(app.element('#shortcuts-dialog').hidden, true);
  assert.equal(app.nativeOverlay(), false);
  assert.equal(app.document.body.dataset.mode, 'work');
  assert.equal(app.savedCompact(), true);
  assert.equal(app.layoutCalls().length, 0);
  assert.equal(app.calls.some(([command]) => command === 'set_compact_sidebar'), false);
});

test('pending compact overlay open completes before native activation close, with no sustained blank chat', async () => {
  const app = await boot({ diagnostics: { compactSidebar: true }, pendingOverlays: true });
  const opening = app.click('#settings-button');
  await flush();
  assert.equal(app.pendingOverlays.length, 1);
  assert.equal(app.pendingOverlays[0].open, true);
  // A next-unread action with no target preserves the current account but must
  // still dismiss settings and reveal its chat.
  app.emit('workspace-mode', 'personal');
  await flush();
  assert.equal(app.element('#settings-dialog').hidden, true);
  assert.equal(app.pendingOverlays.length, 1, 'close waits behind the outstanding open request');
  app.pendingOverlays[0].complete();
  await flush();
  assert.equal(app.pendingOverlays.length, 2);
  assert.equal(app.pendingOverlays[1].open, false);
  app.pendingOverlays[1].complete();
  await opening;
  await flush();
  assert.equal(app.nativeOverlay(), false);
  assert.equal(app.element('#settings-dialog').hidden, true);
  assert.equal(app.document.body.dataset.mode, 'personal');
  assert.equal(app.savedCompact(), true);
  assert.equal(app.layoutCalls().length, 0);
});

test('failed compact and overlay commands leave preferences intact and offer a retry', async () => {
  const app = await boot({ compactFail: true });
  await app.click('#sidebar-toggle');
  assert.equal(app.document.body.dataset.compact, 'false');
  assert.equal(app.savedCompact(), false);
  assert.match(app.element('#toast').textContent, /Could not change the sidebar/);
  const compactApp = await boot({ diagnostics: { compactSidebar: true }, overlayFail: true });
  await compactApp.click('#settings-button');
  assert.equal(compactApp.element('#settings-dialog').hidden, true);
  assert.equal(compactApp.savedCompact(), true);
  assert.match(compactApp.element('#toast').textContent, /Could not (open settings|restore the account view)/);
});

test('shortcut help reports actual registration and the settings panel never registers shortcuts automatically', async () => {
  const app = await boot({ diagnostics: { shortcuts: {
    summon: { requestedAccelerator: 'Ctrl+Alt+W', accelerator: '', registered: false, error: 'This shortcut is already in use.', additionalAccelerators: [] },
    nextUnread: { requestedAccelerator: 'Ctrl+Alt+U', accelerator: 'Ctrl+Alt+U', registered: true, error: null, additionalAccelerators: [] },
  } } });
  assert.equal(app.element('#shortcut-help-button').hidden, false);
  assert.equal(app.element('#summon-shortcut-status').textContent, 'This shortcut is already in use.');
  assert.equal(app.element('#summon-shortcut-status').dataset.state, 'unavailable');
  assert.match(app.element('#shortcut-unread-help').textContent, /Ctrl\+Alt\+U/);
  await app.click('#shortcuts-button');
  assert.equal(app.element('#shortcuts-dialog').hidden, false);
  assert.equal(app.element('#summon-shortcut').value, 'Ctrl+Alt+W');
  assert.equal(app.calls.some(([command]) => command === 'configure_shortcuts'), false);
});

test('shortcut presets and disabled roles save only after submit and reflect partial registration failures', async () => {
  const app = await boot({ shortcutConflicts: ['Ctrl+Alt+Space'] });
  await app.click('#shortcuts-button');
  app.element('#summon-shortcut').value = 'Ctrl+Alt+Space';
  app.element('#unread-shortcut').value = '';
  await app.submitShortcuts();
  assert.deepEqual(app.calls.filter(([command]) => command === 'configure_shortcuts'), [
    ['configure_shortcuts', { summon: 'Ctrl+Alt+Space', nextUnread: '' }],
  ]);
  assert.equal(app.element('#summon-shortcut-status').dataset.state, 'unavailable');
  assert.equal(app.element('#summon-shortcut-status').textContent, 'This shortcut is already in use. Still using Ctrl+Alt+W.');
  assert.match(app.element('#shortcut-summon-help').textContent, /Ctrl\+Alt\+W/);
  assert.equal(app.element('#summon-shortcut').value, 'Ctrl+Alt+Space', 'editor shows the failed requested key, not the retained actual key');
  assert.equal(app.element('#unread-shortcut-status').textContent, 'Shortcut disabled');
  assert.equal(app.element('#shortcuts-save').disabled, false);
});

test('shortcut unregister failures show owned fallback keys rather than claiming Disabled', async () => {
  const app = await boot({ diagnostics: { shortcuts: {
    summon: {
      requestedAccelerator: '', accelerator: 'Ctrl+Alt+Space', registered: true,
      error: 'Windows could not release the previous shortcut.', additionalAccelerators: ['Ctrl+Alt+W'],
    },
    nextUnread: { requestedAccelerator: '', accelerator: '', registered: false, error: null, additionalAccelerators: [] },
  } } });
  await app.click('#shortcuts-button');
  assert.match(app.element('#summon-shortcut-status').textContent, /Still using Ctrl\+Alt\+Space/);
  assert.match(app.element('#summon-shortcut-status').textContent, /Also active: Ctrl\+Alt\+W/);
  assert.equal(app.element('#summon-shortcut').value, '', 'disabled remains the requested preference while actual active keys are disclosed');
  assert.notEqual(app.element('#summon-shortcut-status').textContent, 'Shortcut disabled');
  assert.equal(app.element('#unread-shortcut-status').textContent, 'Shortcut disabled');
});
