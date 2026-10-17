'use strict';
const byId = id => document.getElementById(id);
const node = (tag, text, cls) => {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = String(text);
  if (cls) element.className = cls;
  return element;
};
const short = hash => hash.length > 24 ? hash.slice(0, 14) + '…' + hash.slice(-8) : hash;
const link = (text, href, cls) => { const a = node('a', text, cls); a.href = href; return a; };
const hashLink = (hash, kind, text) => { const a = link(text || short(hash), '/' + kind + '/' + encodeURIComponent(hash), 'mono hash-link'); a.title = hash; return a; };
const count = value => { try { return BigInt(value).toLocaleString('en-US'); } catch { return String(value); } };
const resources = value => Array.isArray(value) ? value.map(count).join(' / ') : '—';
// A count in at most three figures and an SI prefix (1.42M for 1,422,091),
// cut rather than rounded so that it never shows more than there is; below
// ten thousand, the count itself.
const PREFIXES = ['', 'k', 'M', 'G', 'T', 'P', 'E', 'Z', 'Y', 'R', 'Q'];
function compact(value) {
  let digits;
  try { digits = BigInt(value).toString(); } catch { return String(value); }
  if (digits.startsWith('-') || digits.length <= 4) return count(value);
  const prefix = Math.floor((digits.length - 1) / 3);
  if (prefix >= PREFIXES.length) {
    const tail = digits.slice(1, 3).replace(/0+$/, '');
    return digits[0] + (tail ? '.' + tail : '') + 'e' + (digits.length - 1);
  }
  const whole = digits.length - prefix * 3;
  const fraction = digits.slice(whole, 3).replace(/0+$/, '');
  return digits.slice(0, whole) + (fraction ? '.' + fraction : '') + PREFIXES[prefix];
}
// A compact count with the full one on hover.
function compactNode(value, cls) {
  const element = node('span', compact(value), cls);
  if (element.textContent !== count(value)) element.title = count(value);
  return element;
}
// A count that, too long for its line, breaks only after a group separator.
function grouped(value, cls) {
  const element = node('span', undefined, cls), parts = count(value).split(',');
  parts.forEach((part, index) => {
    element.append(index < parts.length - 1 ? part + ',' : part);
    if (index < parts.length - 1) element.append(document.createElement('wbr'));
  });
  return element;
}
// A share of a capacity, rounded down so that only a full block shows 100%.
const share = (used, limit) => {
  const total = Number(limit);
  if (!(total > 0)) return null;
  const percent = Number(used) / total * 100;
  if (percent === 0) return 0;
  return Math.min(percent, 100);
};
const percentText = percent => percent === 0 ? '0%' : percent < 0.1 ? '<0.1%' :
  (percent < 10 ? (Math.floor(percent * 10) / 10).toFixed(1) : String(Math.floor(percent))) + '%';
// One resource's use, compact, beside its share of the block's capacity; the
// full amounts on hover.
function usageCell(used, limit) {
  const cell = node('span', undefined, 'usage'), percent = share(used, limit);
  cell.append(node('span', compact(used), 'usage-value'));
  if (percent !== null) cell.append(node('span', percentText(percent), 'usage-share'));
  cell.title = count(used) + (percent === null ? '' : ' of ' + count(limit));
  return cell;
}
// A block's use of each resource against its capacity.
function usageList(used, limits) {
  if (!Array.isArray(used)) return node('span', '—');
  const list = node('div', undefined, 'usage-list');
  RESOURCES.forEach(([symbol, name], index) => {
    const percent = Array.isArray(limits) ? share(used[index], limits[index]) : null;
    const row = node('div', undefined, 'usage-row'); row.title = name;
    row.append(node('span', symbol, 'usage-symbol'),
      node('span', count(used[index]) + (percent === null ? '' : ' of ' + count(limits[index])), 'mono'));
    if (percent !== null) row.append(node('span', percentText(percent), 'usage-share'));
    list.append(row);
  });
  return list;
}
const pill = (text, tone) => node('span', text, 'badge' + (tone ? ' ' + tone : ''));
const badge = receipt => pill(receipt.status_name, receipt.status === 0 ? 'success' : 'failed');
const RESOURCES = [['C', 'Compute'], ['S', 'State access'], ['D', 'Data'], ['G', 'Growth']];
// The resource columns of a block table, each a share of the block's capacity.
const RESOURCE_COLUMNS = [['Compute', 'Compute'], ['State', 'State access'], ['Data', 'Data'], ['Growth', 'Growth']]
  .map(([label, name]) => ({label, cls: 'num', title: name + ' used, and its share of the block\u2019s capacity'}));
const timestamp = seconds => {
  const date = new Date(Number(seconds) * 1000);
  return Number.isNaN(date.getTime()) ? String(seconds) : date.toISOString().replace('T', ' ').replace('.000Z', '');
};
const segments = location.pathname.split('/').filter(Boolean);
const kind = segments[0] || 'overview', target = segments[1] === undefined ? undefined : decodeURIComponent(segments[1]);
async function json(path) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 15000);
  try {
    const response = await fetch(path, {cache: 'no-store', signal: abort.signal});
    const value = await response.json();
    if (!response.ok) {
      const error = new Error(value.error || ('Request failed: ' + response.status));
      error.status = response.status;
      throw error;
    }
    return value;
  } finally { clearTimeout(timeout); }
}
function setTitle(text, label) {
  byId('page-title').textContent = text;
  byId('page-badge').replaceChildren(...(label ? [label] : []));
  document.title = text + ' · Ting';
}
// A table; a header is its label, or {label, cls, title} with a class its
// column's cells take too.
function table(headers, rows) {
  const wrapper = node('div', undefined, 'table-scroll');
  const grid = node('table'), head = node('thead'), heading = node('tr'), body = node('tbody');
  const columns = headers.map(header => typeof header === 'string' ? {label: header} : header);
  for (const column of columns) { const th = node('th', column.label, column.cls); if (column.title) th.title = column.title; heading.append(th); }
  head.append(heading);
  for (const values of rows) {
    const row = node('tr');
    values.forEach((value, index) => {
      const cell = node('td', undefined, columns[index] && columns[index].cls);
      cell.append(value instanceof Node ? value : node('span', value)); row.append(cell);
    });
    body.append(row);
  }
  grid.append(head, body); wrapper.append(grid); return wrapper;
}
function details(entries) {
  const container = node('div', undefined, 'details');
  for (const [label, value, mono] of entries) {
    const row = node('div', undefined, 'detail-row'), content = node('div', undefined, 'detail-value' + (mono ? ' mono' : ''));
    content.append(value instanceof Node ? value : node('span', value));
    row.append(node('div', label, 'detail-label'), content); container.append(row);
  }
  return container;
}
function raw(label, value) {
  const disclosure = node('details'); disclosure.append(node('summary', label));
  disclosure.addEventListener('toggle', () => {
    if (disclosure.open && disclosure.children.length === 1) disclosure.append(node('pre', value === null ? 'Larger than 1 MiB: read it from its block in slices.' : typeof value === 'string' ? value : JSON.stringify(value, null, 2)));
  });
  return disclosure;
}
function heading(title, action, level) {
  const row = node('div', undefined, 'section-heading'); row.append(node(level || 'h2', title));
  if (action) row.append(action); return row;
}
function empty(text) { return node('p', text, 'empty'); }
function errorView(error) { const box = node('div', undefined, 'error'); box.append(node('h2', 'Unable to load this view'), node('p', error.message), link('Return to the overview', '/')); return box; }
function stats(status) {
  const grid = node('section', undefined, 'stats-grid'); grid.setAttribute('aria-label', 'Chain summary');
  // A card shows a count, or any other element as it is.
  // A card shows a figure, or any other element as it is.
  const card = (label, value, note) => {
    const article = node('article', undefined, 'stat'); article.append(node('p', label, 'stat-label'));
    if (value instanceof Element && value.tagName !== 'SPAN') article.append(value);
    else { const figure = node('p', undefined, 'stat-value'); figure.append(value); article.append(figure); }
    if (note) { const line = node('p', undefined, 'stat-note'); line.append(note); article.append(line); }
    return article;
  };
  const limits = node('dl', undefined, 'capacity');
  RESOURCES.forEach(([symbol, name], index) => {
    const item = node('div'), value = node('dd'); item.title = name;
    value.append(Array.isArray(status.limits) ? compactNode(status.limits[index]) : '—');
    item.append(node('dt', symbol), value); limits.append(item);
  });
  const pendingBytes = compactNode(status.mempool_bytes); pendingBytes.append(' bytes');
  grid.append(card('Height', grouped(status.height)), card('Chain work', compactNode(status.work)),
    card('Pending', grouped(status.mempool_count), pendingBytes), card('Block capacity', limits));
  return grid;
}
function blockTable(blocks, emptyText) {
  if (!blocks.length) return empty(emptyText);
  return table(['Height', 'Block hash', {label: 'Time (UTC)', cls: 'nowrap'}, {label: 'Transactions', cls: 'num'}, ...RESOURCE_COLUMNS], blocks.slice().reverse().map(block => [
    link(count(block.height), '/blocks/' + block.height), hashLink(block.id, 'blocks'),
    timestamp(block.timestamp), count(block.transactions.length),
    ...RESOURCES.map((_, index) => usageCell(block.used[index], Array.isArray(block.limits) ? block.limits[index] : undefined))
  ]));
}
function staleNote(container, observed, status) {
  if (observed !== status.tip) container.append(node('p', 'The chain changed while this page loaded. Refresh to follow the latest canonical branch.', 'muted'));
}
function eventsView(receipts) {
  const events = receipts.flatMap((receipt, index) => receipt.events.map(event => ({...event, transaction: receipt.transaction, index})));
  const section = node('section'); section.append(heading('Events (' + events.length + ')'));
  if (events.length) {
    section.append(table(['Transaction', 'Contract', 'Payload (hex)'], events.slice(0, 200).map(event => [
      hashLink(event.transaction, 'transactions'), hashLink(event.contract, 'contracts'), raw(event.payload_hex ? short(event.payload_hex) : 'Empty payload', event.payload_hex)
    ])));
    if (events.length > 200) section.append(node('p', 'Showing the first 200 events. The API provides the complete block data.', 'muted'));
  } else section.append(empty('No events.'));
  return section;
}
function envelope(summary) {
  if (!summary) return node('p', 'Envelope summary unavailable.', 'muted');
  const entries = [['Kind', summary.kind], ['Budget C / S / D / G', resources(summary.budget), true], ['Envelope bytes', count(summary.encoded_bytes)], ['Input bytes', count(summary.input_bytes)]];
  if (summary.target) entries.push(['Target contract', hashLink(summary.target, 'contracts')]);
  if (summary.code_bytes !== null) entries.push(['Original code bytes', count(summary.code_bytes)]);
  return details(entries);
}
async function overviewView(status) {
  setTitle('Overview');
  const page = await json('/v1/blocks?from=' + Math.max(1, status.height - 9) + '&limit=10');
  const container = node('div');
  container.append(stats(status), heading('Recent blocks', status.height > 0 ? link('All blocks →', '/blocks') : undefined), blockTable(page.items, 'No blocks mined yet.'));
  container.append(heading('Chain'), details([
    ['Chain ID', status.chain_id, true],
    ['Canonical tip', status.height === 0 ? node('span', status.tip + ' (genesis anchor)', 'mono') : hashLink(status.tip, 'blocks', status.tip)],
    ['State root', status.state_root, true]
  ]));
  staleNote(container, page.tip, status);
  return container;
}
async function blocksView(status) {
  setTitle('Blocks');
  const selected = new URLSearchParams(location.search).get('from');
  const from = selected === null ? Math.max(1, status.height - 19) : Number(selected);
  if (!Number.isSafeInteger(from) || from < 1) throw Error('Block pagination starts at a positive height.');
  const page = await json('/v1/blocks?from=' + from + '&limit=20');
  const container = node('div'), pagination = node('nav', undefined, 'pagination');
  pagination.setAttribute('aria-label', 'Block pages');
  pagination.append(from > 1 ? link('← Older', '/blocks?from=' + Math.max(1, from - 20)) : node('span', 'Beginning of chain', 'muted'));
  pagination.append(node('span', 'Heights ' + count(from) + '–' + count(Math.max(from, page.next - 1)), 'muted'));
  pagination.append(page.next <= status.height ? link('Newer →', '/blocks?from=' + page.next) : link('Latest', '/blocks'));
  container.append(blockTable(page.items, 'No mined blocks in this range.'), pagination);
  staleNote(container, page.tip, status);
  return container;
}
async function blockView(id) {
  const block = await json('/v1/blocks/' + encodeURIComponent(id));
  setTitle('Block ' + count(block.height), block.canonical ? pill('Canonical') : pill('Side branch', 'warn'));
  const container = node('div');
  container.append(details([
    ['Block hash', block.id, true], ['Parent', block.height === 1 ? link('Genesis anchor', '/') : hashLink(block.parent, 'blocks')],
    ['Timestamp (UTC)', timestamp(block.timestamp)], ['Branch work', count(block.work), true], ['Capacity used', usageList(block.used, block.limits)],
    ['Serialized bytes', count(block.bytes)], ['Nonce', block.nonce, true], ['Difficulty target', block.target, true],
    ['Votes C / S / D / G', resources(block.votes), true], ['State root', block.state_root, true],
    ['Transactions root', block.transactions_root, true], ['Receipts root', block.receipts_root, true]
  ]));
  container.append(heading('Transactions (' + block.transactions.length + ')'));
  if (block.transactions.length) {
    container.append(table(['Index', 'Transaction ID', 'Outcome', 'Used C / S / D / G', {label: 'Result bytes', cls: 'num'}], block.transactions.slice(0, 200).map((tx, index) => {
      const receipt = block.receipts[index]; return [count(index), hashLink(tx, 'transactions'), badge(receipt), node('span', resources(receipt.used), 'mono'), count(receipt.result_hex.length / 2)];
    })));
    if (block.transactions.length > 200) container.append(node('p', 'Showing the first 200 transactions. The API provides the complete block data.', 'muted'));
  } else container.append(empty('No transactions.'));
  const data = node('div', undefined, 'raw-data');
  data.append(raw('Producer data (hex)', block.producer_hex), raw('Complete block response', block), link('Canonical block bytes (JSON)', '/v1/blocks/' + encodeURIComponent(block.id) + '/raw'));
  container.append(eventsView(block.receipts), heading('Raw data'), data);
  return container;
}
async function transactionView(id, status) {
  const offset = new URLSearchParams(location.search).get('from') || '0';
  const tx = await json('/v1/transactions/' + encodeURIComponent(id) + '?from=' + encodeURIComponent(offset) + '&limit=20');
  watchPending = tx.pending;
  setTitle('Transaction', tx.pending ? pill('Pending', 'warn') : undefined);
  const container = node('div'); container.append(details([['Transaction ID', tx.id, true]]));
  if (tx.pending_transaction) container.append(heading('Pending envelope'), envelope(tx.pending_transaction.transaction), raw('Pending canonical envelope (hex)', tx.pending_transaction.hex));
  container.append(heading('Canonical occurrences'));
  if (!tx.occurrences.length) container.append(empty(tx.pending ? 'Awaiting block inclusion.' : 'No occurrences in this page.'));
  for (const occurrence of tx.occurrences) {
    const receipt = occurrence.receipt, item = node('section', undefined, 'occurrence');
    item.append(heading('Height ' + count(occurrence.height) + ' · transaction ' + count(occurrence.index), badge(receipt), 'h3'), details([
      ['Block', hashLink(occurrence.block, 'blocks')], ['Used C / S / D / G', resources(receipt.used), true], ['Result bytes', count(receipt.result_hex.length / 2)]
    ]), envelope(occurrence.transaction));
    if (receipt.status === 0 && occurrence.transaction && occurrence.transaction.kind === 'deploy' && receipt.result_hex.length === 64) {
      item.append(details([['Deployed contract', hashLink(receipt.result_hex, 'contracts')]]));
    }
    item.append(raw('Result (hex)', receipt.result_hex), eventsView([receipt]), raw('Canonical envelope (hex)', occurrence.hex));
    container.append(item);
  }
  if (tx.has_more) container.append(link('More occurrences →', '/transactions/' + encodeURIComponent(id) + '?from=' + tx.next, 'button'));
  staleNote(container, tx.tip, status);
  return container;
}
async function poolView() {
  setTitle('Pending transactions');
  const pool = await json('/v1/mempool'), container = node('div');
  container.append(node('p', count(pool.items.length) + ' transactions · ' + count(pool.bytes) + ' bytes', 'muted lead'));
  container.append(pool.items.length ? table(['Transaction ID', {label: 'Envelope bytes', cls: 'num'}, 'Budget C / S / D / G'], pool.items.map(tx => [hashLink(tx.id, 'transactions'), count(tx.bytes), node('span', resources(tx.budget), 'mono')])) : empty('The transaction pool is empty.'));
  return container;
}
async function contractView(id) {
  const contract = await json('/v1/contracts/' + encodeURIComponent(id)), container = node('div');
  setTitle('Contract');
  container.append(details([['Contract ID', contract.id, true], ['Original code hash', contract.code_hash, true], ['Code bytes', count(contract.code_bytes)], ['Current state root', contract.state_root, true]]));
  container.append(heading('Storage'));
  const form = node('form', undefined, 'storage-form'), input = node('input'), button = node('button', 'Read record');
  input.id = 'storage-key'; input.placeholder = '32-byte storage key (64 hex characters)'; input.required = true; input.pattern = '[0-9a-fA-F]{64}'; input.spellcheck = false; input.autocomplete = 'off'; input.setAttribute('aria-label', 'Storage key');
  button.type = 'submit'; form.append(input, button); const result = node('div');
  form.addEventListener('submit', async event => {
    event.preventDefault(); button.disabled = true;
    try {
      const value = await json('/v1/contracts/' + encodeURIComponent(id) + '/storage/' + input.value.trim().toLowerCase());
      result.replaceChildren(details([['Record', value.value_hex === null ? 'Absent' : value.value_hex === '' ? 'Present, empty value' : 'Present'], ['Value (hex)', value.value_hex === null ? '—' : value.value_hex, true], ['State root', value.state_root, true]]));
    } catch (error) { result.replaceChildren(errorView(error)); } finally { button.disabled = false; }
  });
  container.append(form, result);
  return container;
}
function render(status) {
  if (kind === 'overview') return overviewView(status);
  if (kind === 'blocks' && target === undefined) return blocksView(status);
  if (kind === 'blocks') return blockView(target);
  if (kind === 'transactions') return transactionView(target, status);
  if (kind === 'contracts') return contractView(target);
  if (kind === 'mempool') return poolView();
  throw Error('Unknown explorer page.');
}
function setState(state, text) {
  const line = document.querySelector('.status-line');
  if (line.dataset.state !== state) { line.dataset.state = state; byId('status-text').textContent = text; }
}
let shownTip = '';
function showStatus(status) {
  setState('online', 'Connected');
  byId('status-network').textContent = {production: 'Ting', development: 'Development chain'}[status.network] || 'Other network';
  byId('status-height').textContent = count(status.height);
  if (status.tip !== shownTip) {
    shownTip = status.tip;
    const label = status.tip.slice(0, 10) + '…' + status.tip.slice(-6);
    byId('status-tip').replaceChildren(status.height === 0 ? node('span', 'Genesis anchor') : hashLink(status.tip, 'blocks', label));
  }
  const pending = byId('nav-pending');
  pending.hidden = !status.mempool_count; pending.textContent = count(status.mempool_count);
}
let refreshing = false, watchPending = false, rendered = '';
async function refresh(auto) {
  if (refreshing) return; refreshing = true;
  try {
    let status;
    try { status = await json('/v1/status'); } catch (error) {
      setState('offline', 'Unavailable');
      if (!rendered) byId('view').replaceChildren(errorView(error));
      return;
    }
    showStatus(status);
    const observed = status.tip + ' ' + status.mempool_count;
    if (auto && observed === rendered) return;
    let content;
    try { content = await render(status); } catch (error) { content = errorView(error); }
    byId('view').replaceChildren(content); rendered = observed;
  } finally { refreshing = false; }
}
setTitle(kind === 'blocks' && target !== undefined ? (/^[0-9]+$/.test(target) ? 'Block ' + target : 'Block') :
  ({overview: 'Overview', blocks: 'Blocks', transactions: 'Transaction', contracts: 'Contract', mempool: 'Pending transactions'})[kind] || 'Not found');
document.querySelectorAll('[data-view]').forEach(a => { if (a.dataset.view === kind) a.setAttribute('aria-current', 'page'); });
byId('search-query').addEventListener('input', () => { byId('search-message').textContent = ''; });
byId('search').addEventListener('submit', async event => {
  event.preventDefault(); const value = byId('search-query').value.trim().toLowerCase(); const message = byId('search-message');
  if (/^[0-9]+$/.test(value) && value.length < 20) { location.href = '/blocks/' + encodeURIComponent(value); return; }
  if (!/^[0-9a-f]{64}$/.test(value)) { message.textContent = 'Enter a block height or a 64-character identifier.'; return; }
  const button = event.currentTarget.querySelector('button'); button.disabled = true; message.textContent = 'Searching…';
  try {
    for (const kind of ['blocks', 'transactions', 'contracts']) {
      try { await json('/v1/' + kind + '/' + value); location.href = '/' + kind + '/' + value; return; }
      catch (error) { if (error.status !== 404) throw error; }
    }
    message.textContent = 'No matching block, transaction or contract in this node’s view.';
  } catch (error) { message.textContent = error.message; } finally { button.disabled = false; }
});
refresh(false);
setInterval(() => { if (kind === 'overview' || kind === 'mempool' || watchPending) refresh(true); }, 5000);
