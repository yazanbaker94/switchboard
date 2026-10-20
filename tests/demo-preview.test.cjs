const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { once } = require('node:events');
const { createDemoServer, renderDemoHtml } = require('./demo-preview.cjs');

test('demo wraps the actual shell with a permanent fictional-content label', () => {
  const html = renderDemoHtml();
  assert.match(html, /id="sidebar-toggle"/);
  assert.match(html, /id="shortcuts-form"/);
  assert.match(html, /DEMO · FICTIONAL CONVERSATIONS/);
  assert.match(html, /src="\/demo-bridge\.js" defer/);
  assert.match(html, /src="\/app\.js" defer/);
});

test('local demo serves actual UI assets and cannot expose arbitrary files or external connections', async (t) => {
  const server = createDemoServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const base = `http://127.0.0.1:${server.address().port}`;
  for (const file of ['styles.css', 'app.js']) {
    const response = await fetch(`${base}/${file}`);
    assert.equal(response.status, 200);
    assert.equal(await response.text(), fs.readFileSync(path.join(__dirname, '..', 'ui', file), 'utf8'));
    assert.match(response.headers.get('content-security-policy'), /connect-src 'none'/);
  }
  for (const view of ['split', 'compact', 'shortcuts']) {
    const response = await fetch(`${base}/?view=${view}`);
    assert.equal(response.status, 200);
    assert.match(await response.text(), /DEMO · FICTIONAL CONVERSATIONS/);
  }
  for (const route of ['/package.json', '/ui/app.js', '/.tools/diagnostics.json', '/profiles/personal']) {
    assert.equal((await fetch(`${base}${route}`)).status, 404);
  }
  const bridge = await (await fetch(`${base}/demo-bridge.js`)).text();
  assert.match(bridge, /Object\.defineProperty\(window, 'localStorage'/);
  assert.match(bridge, /Fictional messages created for this preview/);
  assert.doesNotMatch(bridge, /https?:\/\//);
});
