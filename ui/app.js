(() => {
  'use strict';

  const accounts = ['personal', 'work'];
  const defaults = { personal: { name: 'Personal', label: '' }, work: { name: 'Work', label: '' } };
  const storageKey = 'switchboard.labels.v1';
  const invoke = window.__TAURI__?.core?.invoke;
  let labels = structuredClone(defaults);
  let mode = 'personal';
  let lastSingleAccount = 'personal';
  let modeRevision = 0;
  let compactSidebar = false;
  let compactRevision = 0;
  let compactSaving = false;
  let activePanel = null;
  let overlayDesired = false;
  let overlayRequest = Promise.resolve(true);
  let overlayRevision = 0;
  let shortcuts = null;
  let shortcutRevision = 0;
  let shortcutsSaving = false;
  let toastTimer;
  const loadStates = { personal: 'unknown', work: 'unknown' };
  const loadRevisions = { personal: 0, work: 0 };
  const loadTimers = {};
  const unreadCounts = { personal: null, work: null };
  const unreadRevisions = { personal: 0, work: 0 };
  const notificationPermissions = { personal: 'unknown', work: 'unknown' };
  const notificationBusy = { personal: false, work: false };
  const notificationGranting = { personal: false, work: false };
  const notificationErrors = { personal: '', work: '' };
  const notificationRevisions = { personal: 0, work: 0 };
  let permissionQuery = 0;

  try {
    const stored = JSON.parse(localStorage.getItem(storageKey) || 'null');
    for (const account of accounts) {
      if (stored?.[account]) {
        labels[account].name = typeof stored[account].name === 'string' ? stored[account].name.trim().slice(0, 28) || defaults[account].name : defaults[account].name;
        labels[account].label = typeof stored[account].label === 'string' ? stored[account].label.trim().slice(0, 24) : '';
      }
    }
  } catch { /* A blocked or invalid preference store should not prevent opening WhatsApp. */ }

  const $ = (selector) => document.querySelector(selector);

  function announce(message) {
    $('#announcer').textContent = message;
  }

  function toast(message) {
    clearTimeout(toastTimer);
    $('#toast').textContent = message;
    $('#toast').hidden = false;
    toastTimer = setTimeout(() => { $('#toast').hidden = true; }, 6000);
  }

  function renderCompact() {
    document.body.dataset.compact = String(compactSidebar);
    const button = $('#sidebar-toggle');
    const label = `${compactSidebar ? 'Expand' : 'Collapse'} account sidebar`;
    button.setAttribute('aria-expanded', String(!compactSidebar));
    button.setAttribute('aria-label', label);
    button.title = label;
    button.disabled = compactSaving;
  }

  function renderPanels() {
    for (const [panel, dialog, button] of [
      ['labels', '#settings-dialog', '#settings-button'],
      ['alerts', '#notifications-dialog', '#notifications-button'],
      ['shortcuts', '#shortcuts-dialog', '#shortcuts-button'],
    ]) {
      $(dialog).hidden = activePanel !== panel;
      $(button).setAttribute('aria-expanded', String(activePanel === panel));
    }
    $('#shortcut-help-button').setAttribute('aria-expanded', String(activePanel === 'shortcuts'));
  }

  function syncPanelOverlay() {
    const desired = compactSidebar && activePanel !== null;
    if (desired === overlayDesired) return overlayRequest;
    overlayDesired = desired;
    const revision = ++overlayRevision;
    if (!invoke) return Promise.resolve(true);
    const previousRequest = overlayRequest;
    // Overlay mutations depend on one another. In particular, a pending open
    // must finish before a native activation's close can reveal the chat.
    overlayRequest = previousRequest.catch(() => false).then(() => invoke('set_overlay', { open: desired })).then(() => true).catch((error) => {
      console.error('Could not update settings overlay:', error);
      if (revision === overlayRevision) {
        activePanel = null;
        renderPanels();
        overlayDesired = null;
        // A failed open may have partially hidden native views. Queue one close
        // after this request settles; a failed close stays retryable, without a loop.
        if (desired) Promise.resolve().then(() => syncPanelOverlay());
        toast(desired ? 'Could not open settings. Please try again.' : 'Could not restore the account view. Select an account to try again.');
      }
      return false;
    });
    return overlayRequest;
  }

  async function setPanel(panel) {
    activePanel = panel;
    renderPanels();
    return syncPanelOverlay();
  }

  function applyCompact(value) {
    if (typeof value !== 'boolean') return;
    compactRevision += 1;
    compactSidebar = value;
    renderCompact();
    syncPanelOverlay();
  }

  async function changeCompact(value) {
    compactRevision += 1;
    compactSaving = true;
    renderCompact();
    const requestedRevision = compactRevision;
    try {
      if (invoke) await invoke('set_compact_sidebar', { compact: value });
      else {
        try { localStorage.setItem('switchboard.compact.preview', String(value)); } catch { /* Preview preference is optional. */ }
      }
      if (compactRevision === requestedRevision) applyCompact(value);
      return true;
    } catch (error) {
      console.error('Could not update sidebar preference:', error);
      toast('Could not change the sidebar. Please try again.');
      return false;
    } finally {
      compactSaving = false;
      renderCompact();
    }
  }

  function applyUnread(account, value) {
    if (!accounts.includes(account)) return;
    unreadRevisions[account] += 1;
    unreadCounts[account] = Number.isSafeInteger(value) && value >= 0 ? value : null;
    renderUnread();
  }

  function applyShortcuts(value) {
    if (!value || typeof value !== 'object') return;
    shortcutRevision += 1;
    shortcuts = {};
    for (const role of ['summon', 'nextUnread']) {
      const status = value[role];
      shortcuts[role] = status && typeof status === 'object' ? {
        registered: status.registered === true,
        accelerator: typeof status.accelerator === 'string' ? status.accelerator : '',
        requestedAccelerator: typeof status.requestedAccelerator === 'string'
          ? status.requestedAccelerator : typeof status.accelerator === 'string' ? status.accelerator : '',
        additionalAccelerators: Array.isArray(status.additionalAccelerators)
          ? status.additionalAccelerators.filter((value) => typeof value === 'string' && value.length > 0) : [],
        error: typeof status.error === 'string' ? status.error : '',
      } : null;
    }
    renderShortcuts();
  }

  function renderShortcuts() {
    const known = shortcuts !== null;
    $('#shortcut-help-button').hidden = !known;
    for (const [role, selector, help, action] of [
      ['summon', '#summon-shortcut-status', '#shortcut-summon-help', 'Show / hide'],
      ['nextUnread', '#unread-shortcut-status', '#shortcut-unread-help', 'Next unread account'],
    ]) {
      const status = shortcuts?.[role];
      const registered = status?.registered === true;
      const disabled = status && !registered && !status.accelerator && !status.requestedAccelerator
        && status.additionalAccelerators.length === 0 && !status.error;
      const actual = status?.accelerator || '';
      $(selector).dataset.state = status?.error ? 'unavailable' : registered ? 'registered' : disabled ? 'disabled' : 'unavailable';
      $(selector).textContent = !status ? 'Availability not checked'
        : status.error ? `${status.error}${registered && actual ? ` Still using ${actual}.` : ''}${status.additionalAccelerators.length ? ` Also active: ${status.additionalAccelerators.join(', ')}.` : ''}`
        : registered ? `Available from any app${status.additionalAccelerators.length ? `. Also active: ${status.additionalAccelerators.join(', ')}.` : ''}`
        : disabled ? 'Shortcut disabled'
        : status.error || 'Windows could not register this shortcut. Choose another.';
      $(help).textContent = registered ? `${actual} · ${action}${status.error ? ' (check settings)' : ''}`
        : `${action}: ${disabled ? 'disabled' : 'unavailable'}`;
    }
    $('#shortcuts-save').disabled = !invoke || !known || shortcutsSaving;
    $('#summon-shortcut').disabled = !known || shortcutsSaving;
    $('#unread-shortcut').disabled = !known || shortcutsSaving;
  }

  function populateShortcutEditors() {
    for (const [role, selector, choices] of [
      ['summon', '#summon-shortcut', ['Ctrl+Alt+W', 'Ctrl+Alt+Space', '']],
      ['nextUnread', '#unread-shortcut', ['Ctrl+Alt+U', 'Ctrl+Alt+N', '']],
    ]) {
      const accelerator = shortcuts?.[role]?.requestedAccelerator ?? shortcuts?.[role]?.accelerator ?? '';
      const choice = choices.find((value) => value.toLowerCase() === accelerator.replace(/\s/g, '').toLowerCase());
      $(selector).value = choice ?? '';
    }
  }

  function renderLabels() {
    for (const account of accounts) {
      document.querySelectorAll(`[data-name="${account}"]`).forEach((element) => { element.textContent = labels[account].name; });
      const button = $(`#account-${account}`);
      button.querySelector('.account-avatar').textContent = Array.from(labels[account].name)[0]?.toUpperCase() || defaults[account].name[0];
      button.title = `${labels[account].name}${labels[account].label ? ` · ${labels[account].label}` : ''} (Ctrl+${account === 'personal' ? '1' : '2'})`;
      button.setAttribute('aria-label', `${labels[account].name}${labels[account].label ? `, ${labels[account].label}` : ''}, WhatsApp Web`);
      $(`[data-detail="${account}"]`).textContent = labels[account].label || 'WhatsApp Web';
    }
    renderUnread();
    renderNotificationPermissions();
    renderMode();
  }

  function renderNotificationPermissions() {
    for (const account of accounts) {
      const granted = notificationPermissions[account] === 'granted';
      const button = $(`#allow-notifications-${account}`);
      const status = $(`[data-notification-status="${account}"]`);
      button.disabled = !invoke || notificationBusy[account] || granted;
      button.classList.toggle('permission-granted', granted);
      button.textContent = notificationBusy[account]
        ? notificationGranting[account] ? 'Allowing…' : 'Checking…'
        : granted ? 'Allowed ✓' : notificationErrors[account] ? 'Try again' : 'Allow notifications';
      button.setAttribute('aria-label', `${granted ? 'Desktop notifications allowed for' : 'Allow desktop notifications for'} ${labels[account].name}`);
      status.textContent = !invoke ? 'Available in the Windows app'
        : notificationErrors[account] || (notificationBusy[account]
          ? notificationGranting[account] ? 'Updating browser permission…' : 'Checking browser permission…'
          : granted ? 'Browser permission allowed'
          : notificationPermissions[account] === 'denied' ? 'Browser permission denied'
          : notificationPermissions[account] === 'default' ? 'Browser permission not allowed'
          : 'Browser permission not checked');
    }
  }

  async function refreshNotificationPermissions() {
    if (!invoke) return renderNotificationPermissions();
    const query = ++permissionQuery;
    const requestedRevisions = { ...notificationRevisions };
    for (const account of accounts) {
      if (!notificationGranting[account]) notificationBusy[account] = true;
      notificationErrors[account] = '';
    }
    renderNotificationPermissions();
    try {
      const permissions = await invoke('notification_permissions');
      if (query !== permissionQuery) return;
      for (const account of accounts) {
        if (requestedRevisions[account] !== notificationRevisions[account]) continue;
        notificationPermissions[account] = ['default', 'granted', 'denied'].includes(permissions?.[account]) ? permissions[account] : 'unknown';
      }
    } catch (error) {
      console.error('Could not read browser notification permissions:', error);
      if (query === permissionQuery) {
        for (const account of accounts) {
          if (requestedRevisions[account] === notificationRevisions[account]) notificationErrors[account] = 'Could not check browser permission.';
        }
      }
    } finally {
      if (query === permissionQuery) {
        for (const account of accounts) {
          if (!notificationGranting[account]) notificationBusy[account] = false;
        }
        renderNotificationPermissions();
      }
    }
  }

  async function allowNotifications(account) {
    // This command is reachable only through the explicit per-account button.
    if (!invoke || notificationBusy[account] || notificationPermissions[account] === 'granted') return;
    notificationRevisions[account] += 1;
    notificationBusy[account] = true;
    notificationGranting[account] = true;
    notificationErrors[account] = '';
    renderNotificationPermissions();
    try {
      await invoke('enable_notifications', { account });
      notificationPermissions[account] = 'granted';
    } catch (error) {
      console.error('Could not allow desktop notifications:', error);
      notificationErrors[account] = 'Could not allow notifications. Try again.';
    } finally {
      notificationGranting[account] = false;
      notificationBusy[account] = false;
      renderNotificationPermissions();
    }
  }

  function renderUnread() {
    for (const account of accounts) {
      const badge = $(`[data-unread="${account}"]`);
      const count = unreadCounts[account];
      badge.hidden = !count;
      badge.textContent = count > 99 ? '99+' : String(count || '');
      badge.title = `${count || 0} unread ${count === 1 ? 'chat' : 'chats'}`;
      badge.setAttribute('aria-label', badge.title);
      const button = $(`#account-${account}`);
      button.setAttribute('aria-label', `${labels[account].name}${labels[account].label ? `, ${labels[account].label}` : ''}, WhatsApp Web${count ? `, ${badge.title}` : ''}`);
    }
  }

  function setLoadingState(account, state) {
    loadRevisions[account] += 1;
    clearTimeout(loadTimers[account]);
    loadStates[account] = state;
    if (state === 'loading') {
      // A page-load event is not an authentication or connectivity signal. If
      // completion was missed, stop the activity indicator without claiming success.
      loadTimers[account] = setTimeout(() => {
        loadStates[account] = 'unknown';
        renderMode();
      }, 30000);
    }
    renderMode();
  }

  function renderMode() {
    document.body.dataset.mode = mode;
    for (const account of accounts) {
      const selected = mode === account || mode === 'split';
      const card = $(`#account-${account}`);
      card.classList.toggle('is-active', selected);
      card.classList.toggle('is-loading', loadStates[account] === 'loading');
      card.setAttribute('aria-pressed', String(selected));
      // Native views remain separate accessibility surfaces. Keep shell loading
      // copy out of the accessibility tree once its view has finished loading.
      $(`[data-pane="${account}"]`).hidden = !selected || (Boolean(invoke) && loadStates[account] !== 'loading');
    }
    const split = mode === 'split';
    $('#split-button').setAttribute('aria-pressed', String(split));
    $('#split-button').title = split ? `Return to ${labels[lastSingleAccount].name}` : 'Show both accounts side by side (Ctrl+3)';
    $('#active-name').textContent = split ? 'Both accounts' : labels[mode].name;
    $('#active-description').textContent = split
      ? `${labels.personal.name} on the left · ${labels.work.name} on the right`
      : labels[mode].label || (mode === 'personal' ? 'Your personal session' : 'Your work session');
    $('#active-description').title = $('#active-description').textContent;
    $('#reload-button').setAttribute('aria-label', split ? 'Reload both accounts' : `Reload ${labels[mode].name}`);
    $('#reload-button').title = split ? 'Reload both accounts' : `Reload ${labels[mode].name}`;
    document.title = `${split ? 'Both accounts' : labels[mode].name} · Switchboard`;
  }

  function sendLayout() {
    if (!invoke) return Promise.resolve();
    const requested = { mode, width: window.innerWidth, height: window.innerHeight };
    return invoke('set_layout', requested).catch((error) => {
      console.error('Could not update account layout:', error);
      toast('Could not open the account view. Try switching accounts again.');
    });
  }

  async function synchronizeLabels() {
    if (!invoke) return true;
    try {
      await invoke('set_labels', { personal: labels.personal.name, work: labels.work.name });
      return true;
    } catch (error) {
      console.error('Could not update native account labels:', error);
      toast('Could not update tray labels. Try saving your labels again.');
      return false;
    }
  }

  function applyMode(nextMode) {
    if (!['personal', 'work', 'split'].includes(nextMode)) return false;
    modeRevision += 1;
    mode = nextMode;
    if (mode !== 'split') lastSingleAccount = mode;
    renderMode();
    announce(mode === 'split' ? 'Both accounts shown side by side' : `${labels[mode].name} selected`);
    return true;
  }

  function selectMode(nextMode) {
    if (applyMode(nextMode)) sendLayout();
  }

  async function toggleSettings(show) {
    if (show) {
      for (const account of accounts) {
        $(`#${account}-name`).value = labels[account].name;
        $(`#${account}-label`).value = labels[account].label;
      }
      if (await setPanel('labels') && activePanel === 'labels') $('#personal-name').focus();
    } else {
      await setPanel(null);
      $('#settings-button').focus();
    }
  }

  async function toggleNotifications(show) {
    if (show) {
      if (!await setPanel('alerts') || activePanel !== 'alerts') return;
      $('#notifications-close').focus();
      await refreshNotificationPermissions();
    } else {
      await setPanel(null);
      $('#notifications-button').focus();
    }
  }

  async function toggleShortcuts(show) {
    if (show) {
      if (!await setPanel('shortcuts') || activePanel !== 'shortcuts') return;
      populateShortcutEditors();
      $('#shortcuts-close').focus();
    } else {
      await setPanel(null);
      $('#shortcuts-button').focus();
    }
  }

  for (const account of accounts) $(`#account-${account}`).addEventListener('click', () => selectMode(account));
  $('#split-button').addEventListener('click', () => selectMode(mode === 'split' ? lastSingleAccount : 'split'));
  $('#reload-button').addEventListener('click', async () => {
    if (!invoke) return toast('Open the desktop app to load WhatsApp Web.');
    const reloadAccounts = mode === 'split' ? accounts : [mode];
    $('#reload-button').disabled = true;
    try {
      for (const account of reloadAccounts) {
        setLoadingState(account, 'loading');
        await invoke('reload_account', { account });
      }
      renderMode();
      announce('Reloading WhatsApp Web');
    } catch (error) {
      for (const account of reloadAccounts) setLoadingState(account, 'unknown');
      console.error('Could not reload account:', error);
      toast('Could not reload WhatsApp Web. Please try again.');
    } finally { $('#reload-button').disabled = false; }
  });

  $('#settings-button').addEventListener('click', () => toggleSettings($('#settings-dialog').hidden));
  $('#settings-close').addEventListener('click', () => toggleSettings(false));
  $('#sidebar-toggle').addEventListener('click', async () => {
    await setPanel(null);
    await changeCompact(!compactSidebar);
  });
  $('#shortcuts-button').addEventListener('click', () => toggleShortcuts(activePanel !== 'shortcuts'));
  $('#shortcut-help-button').addEventListener('click', () => toggleShortcuts(activePanel !== 'shortcuts'));
  $('#shortcuts-close').addEventListener('click', () => toggleShortcuts(false));
  $('#shortcuts-form').addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!invoke || shortcutsSaving || !shortcuts) return;
    shortcutRevision += 1;
    shortcutsSaving = true;
    renderShortcuts();
    try {
      const status = await invoke('configure_shortcuts', {
        summon: $('#summon-shortcut').value,
        nextUnread: $('#unread-shortcut').value,
      });
      applyShortcuts(status);
      populateShortcutEditors();
      toast('Shortcut preferences saved. Availability is shown below.');
    } catch (error) {
      console.error('Could not update shortcuts:', error);
      toast('Could not save shortcuts. Please try again.');
    } finally {
      shortcutsSaving = false;
      renderShortcuts();
    }
  });
  $('#notifications-button').addEventListener('click', () => toggleNotifications($('#notifications-dialog').hidden));
  $('#notifications-close').addEventListener('click', () => toggleNotifications(false));
  for (const account of accounts) {
    $(`#allow-notifications-${account}`).addEventListener('click', () => allowNotifications(account));
  }
  $('#labels-form').addEventListener('submit', async (event) => {
    event.preventDefault();
    for (const account of accounts) {
      labels[account] = {
        name: $(`#${account}-name`).value.trim().slice(0, 28) || defaults[account].name,
        label: $(`#${account}-label`).value.trim().slice(0, 24),
      };
    }
    let saved = true;
    try { localStorage.setItem(storageKey, JSON.stringify(labels)); } catch { saved = false; }
    renderLabels();
    await toggleSettings(false);
    if (await synchronizeLabels()) {
      toast(saved ? 'Account labels saved on this PC.' : 'Labels updated for this window. Your preference store is unavailable.');
    }
  });

  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && activePanel === 'shortcuts') {
      toggleShortcuts(false);
      return;
    }
    if (event.key === 'Escape' && !$('#notifications-dialog').hidden) {
      toggleNotifications(false);
      return;
    }
    if (event.key === 'Escape' && !$('#settings-dialog').hidden) {
      toggleSettings(false);
      return;
    }
    if (event.ctrlKey && !event.altKey && !event.shiftKey && ['1', '2', '3'].includes(event.key)) {
      event.preventDefault();
      if (event.key === '3') selectMode('split');
      else selectMode(event.key === '1' ? 'personal' : 'work');
    }
  });

  async function initialize() {
    renderLabels();
    renderCompact();
    renderPanels();
    renderShortcuts();
    if (window.__TAURI__?.event?.listen) {
      try {
        await Promise.all([
          window.__TAURI__.event.listen('account-load', ({ payload }) => {
            if (accounts.includes(payload?.account) && ['loading', 'loaded'].includes(payload?.state)) {
              setLoadingState(payload.account, payload.state);
            }
          }),
          window.__TAURI__.event.listen('workspace-mode', ({ payload }) => {
            // Native shortcuts and the tray already changed the view geometry.
            // Updating only shell state avoids a set_layout event feedback loop.
            if (['personal', 'work', 'split'].includes(payload) && activePanel !== null) setPanel(null);
            applyMode(payload);
          }),
          window.__TAURI__.event.listen('workspace-compact', ({ payload }) => {
            applyCompact(payload);
          }),
          window.__TAURI__.event.listen('account-unread', ({ payload }) => {
            if (payload) applyUnread(payload.account, payload.count);
          }),
        ]);
      } catch (error) { console.error('Could not subscribe to account loading updates:', error); }
    }
    if (invoke) {
      await synchronizeLabels();
      // Setup already laid out the native views. Read the current mode after
      // subscribing so a startup tray/protocol activation is never reset here.
      const requestedModeRevision = modeRevision;
      const requestedLoadRevisions = { ...loadRevisions };
      const requestedCompactRevision = compactRevision;
      const requestedUnreadRevisions = { ...unreadRevisions };
      const requestedShortcutRevision = shortcutRevision;
      try {
        const diagnostics = await invoke('get_diagnostics');
        if (requestedModeRevision === modeRevision) {
          if (accounts.includes(diagnostics?.lastSingleAccount)) lastSingleAccount = diagnostics.lastSingleAccount;
          applyMode(diagnostics?.mode);
        }
        if (requestedCompactRevision === compactRevision) applyCompact(diagnostics?.compactSidebar);
        if (requestedShortcutRevision === shortcutRevision) applyShortcuts(diagnostics?.shortcuts);
        for (const account of accounts) {
          if (requestedUnreadRevisions[account] === unreadRevisions[account] && diagnostics?.unread && Object.hasOwn(diagnostics.unread, account)) {
            applyUnread(account, diagnostics.unread[account]);
          }
          if (requestedLoadRevisions[account] === loadRevisions[account] && typeof diagnostics?.loaded?.[account] === 'boolean') {
            setLoadingState(account, diagnostics.loaded[account] ? 'loaded' : 'loading');
          }
        }
      } catch (error) {
        console.error('Could not read native workspace state:', error);
        toast('Could not sync the account view. Select an account to continue.');
      }
    }
    else {
      for (const account of accounts) loadStates[account] = 'loaded';
      renderMode();
      document.querySelectorAll('.loading-pane .eyebrow').forEach((element) => { element.textContent = 'SWITCHBOARD DESKTOP PREVIEW'; });
      document.querySelectorAll('.loading-pane h2').forEach((element) => { element.textContent = 'Your accounts go here'; });
      document.querySelectorAll('.loading-pane p').forEach((element) => { element.textContent = 'Open Switchboard on Windows to link your accounts.'; });
      document.querySelectorAll('.loading-line').forEach((element) => { element.hidden = true; });
    }
  }

  initialize();
})();
