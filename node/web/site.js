'use strict';
// What every page has: the theme switch, the live height in the header and
// the search. The node's status is read here every five seconds and handed
// to the page as a 'ting:status' event, or 'ting:offline' when it fails.
(() => {
  const byId = id => document.getElementById(id);
  const root = document.documentElement;
  const dark = () => root.dataset.theme ? root.dataset.theme === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches;
  byId('theme-toggle').addEventListener('click', () => {
    const theme = dark() ? 'light' : 'dark';
    root.dataset.theme = theme;
    try { localStorage.setItem('ting-theme', theme); } catch {}
    document.dispatchEvent(new CustomEvent('ting:theme'));
  });

  const live = byId('live'), text = byId('live-text');
  let height = null;
  async function poll() {
    let status;
    try {
      const response = await fetch('/v1/status', {cache: 'no-store'});
      status = await response.json();
      if (!response.ok) throw Error(status.error || 'Request failed: ' + response.status);
    } catch (error) {
      live.dataset.state = 'offline'; text.textContent = 'Offline';
      document.dispatchEvent(new CustomEvent('ting:offline', {detail: error}));
      return;
    }
    live.dataset.state = status.replaying ? 'replaying' : 'online';
    const label = document.createElement('span');
    label.className = 'live-label'; label.textContent = status.replaying ? 'replaying ' : 'height ';
    text.replaceChildren(label, BigInt(status.height).toLocaleString('en-US'));
    live.title = status.network + ' · tip ' + status.tip;
    // A new block makes the dot pulse once.
    if (height !== null && status.height !== height) {
      live.classList.remove('pulse'); void live.offsetWidth; live.classList.add('pulse');
    }
    height = status.height; window.tingStatus = status;
    document.dispatchEvent(new CustomEvent('ting:status', {detail: status}));
  }
  poll();
  setInterval(poll, 5000);

  // A height goes straight to its block; a 64-character identifier is looked
  // up as a block, then a transaction, then a contract.
  const form = byId('search'), input = byId('search-query'), message = byId('search-message');
  const say = value => { message.textContent = value; };
  input.addEventListener('input', () => say(''));
  input.addEventListener('blur', () => setTimeout(() => say(''), 150));
  document.addEventListener('keydown', event => {
    if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.target.closest('input, textarea, select, [contenteditable]')) return;
    event.preventDefault(); input.focus(); input.select();
  });
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const value = input.value.trim().toLowerCase().replace(/^0x/, '');
    if (/^[0-9]+$/.test(value) && value.length < 20) { location.href = '/blocks/' + encodeURIComponent(value); return; }
    if (!/^[0-9a-f]{64}$/.test(value)) { say('Enter a block height or a 64-character identifier.'); return; }
    say('Searching…');
    try {
      for (const kind of ['blocks', 'transactions', 'contracts']) {
        const response = await fetch('/v1/' + kind + '/' + value, {cache: 'no-store'});
        if (response.ok) { location.href = '/' + kind + '/' + value; return; }
        if (response.status !== 404) throw Error((await response.json()).error || 'Request failed: ' + response.status);
      }
      say('No block, transaction or contract with this identifier in this node’s view.');
    } catch (error) { say(error.message); }
  });
})();
