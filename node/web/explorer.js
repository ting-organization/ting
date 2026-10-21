'use strict';
const byId = id => document.getElementById(id);
const node = (tag, text, cls) => {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = String(text);
  if (cls) element.className = cls;
  return element;
};
const SVG = 'http://www.w3.org/2000/svg';
const svg = (tag, attributes, cls) => {
  const element = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes || {})) element.setAttribute(name, value);
  if (cls) element.setAttribute('class', cls);
  return element;
};
const short = hash => hash.length > 24 ? hash.slice(0, 8) + '…' + hash.slice(-6) : hash;
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
// Bytes in binary units, in at most three figures and cut like compact
// counts: 302 B, 4.5 KiB, 0.97 MiB, 1 MiB.
function size(value) {
  let amount = Number(value), unit = 0;
  if (!(amount >= 1000)) return count(value) + ' B';
  const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  while (amount >= 1000 && unit < UNITS.length - 1) { amount /= 1024; unit++; }
  return (amount < 1 ? Math.floor(amount * 100) / 100 : amount < 10 ? Math.floor(amount * 10) / 10 : Math.floor(amount)) + ' ' + UNITS[unit];
}
// A span of seconds in its two largest units: 42 s, 2 min 12 s, 3 h 4 min.
function duration(seconds) {
  const value = Number(seconds), s = Math.round(Math.abs(value));
  if (!Number.isFinite(value)) return '—';
  const sign = value < 0 ? '−' : '';
  if (s < 60) return sign + s + ' s';
  if (s < 3600) return sign + Math.floor(s / 60) + ' min ' + String(s % 60).padStart(2, '0') + ' s';
  if (s < 86400) return sign + Math.floor(s / 3600) + ' h ' + Math.floor(s % 3600 / 60) + ' min';
  return sign + Math.floor(s / 86400) + ' d ' + Math.floor(s % 86400 / 3600) + ' h';
}
const ago = seconds => { const span = Date.now() / 1000 - Number(seconds); return span < 1 ? 'just now' : duration(span) + ' ago'; };
// An age that the page keeps current between blocks.
function age(seconds, cls) { const element = node('span', ago(seconds), cls); element.dataset.age = seconds; element.title = timestamp(seconds) + ' UTC'; return element; }
const timestamp = seconds => {
  const date = new Date(Number(seconds) * 1000);
  return Number.isNaN(date.getTime()) ? String(seconds) : date.toISOString().replace('T', ' ').replace('.000Z', '');
};
// A share of a capacity, rounded down so that only a full block shows 100%.
const share = (used, limit) => {
  const total = Number(limit);
  if (!(total > 0)) return null;
  const percent = Number(used) / total * 100;
  if (percent === 0) return 0;
  return Math.min(percent, 100);
};
const percentText = percent => percent === 0 ? '0%' : percent < 0.01 ? '<0.01%' : percent < 0.1 ? (Math.floor(percent * 100) / 100) + '%' :
  (percent < 10 ? (Math.floor(percent * 10) / 10).toFixed(1) : String(Math.floor(percent))) + '%';
// A hash rate from work and seconds, in hashes per second.
function rate(work, seconds) {
  if (!(seconds > 0)) return '—';
  const perSecond = Number(work) / seconds;
  if (perSecond >= 10000) return compact(BigInt(Math.floor(perSecond))) + ' H/s';
  return (perSecond < 10 ? perSecond.toFixed(2) : Math.floor(perSecond).toLocaleString('en-US')) + ' H/s';
}
// One resource's use, compact, beside its share of the block's capacity; the
// full amounts on hover.
function usageCell(used, limit) {
  const cell = node('span', undefined, 'usage'), percent = share(used, limit);
  cell.append(node('span', compact(used), 'usage-value'));
  if (percent !== null) cell.append(node('span', percentText(percent), 'usage-share'));
  cell.title = count(used) + (percent === null ? '' : ' of ' + count(limit));
  return cell;
}
const RESOURCES = [['C', 'Compute'], ['S', 'State access'], ['D', 'Data'], ['G', 'Growth']];
// An amount of one resource: data and growth are bytes, the others counts.
const amount = (index, value) => index >= 2 ? size(value) : compact(value);
// A block's resource that is nearest its capacity, as a short meter.
function fullest(block) {
  const shares = RESOURCES.map((_, index) => Array.isArray(block.limits) ? share(block.used[index], block.limits[index]) || 0 : 0);
  const index = shares.indexOf(Math.max(...shares)), cell = node('span', undefined, 'fullest');
  cell.title = RESOURCES[index][1] + ': ' + count(block.used[index]) + (Array.isArray(block.limits) ? ' of ' + count(block.limits[index]) : '');
  cell.append(meter(shares[index], {mini: true}), node('span', RESOURCES[index][0], 'symbol'), percentText(shares[index]));
  return cell;
}
const pill = (text, tone) => node('span', text, 'badge' + (tone ? ' ' + tone : ''));
const badge = receipt => pill(receipt.status_name, receipt.status === 0 ? 'success' : 'failed');
const events = block => block.receipts.reduce((sum, receipt) => sum + receipt.events.length, 0);
const network = status => ({production: 'Production network', development: 'Development chain'})[status.network] || 'Other network';
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

// ---- Page parts ------------------------------------------------------------

function setHead({kicker = '', title, mono = false, badges = [], sub = [], actions = []}) {
  byId('page-kicker').textContent = kicker;
  const heading = byId('page-title'); heading.textContent = title; heading.className = mono ? 'mono' : '';
  byId('page-badge').replaceChildren(...badges);
  byId('page-sub').replaceChildren(...[].concat(sub));
  byId('page-actions').replaceChildren(...actions);
  document.title = [kicker, title].filter(Boolean).join(' ') + ' · Ting';
}
function panel(title, note, ...children) {
  const section = node('section', undefined, 'panel');
  if (title) {
    const head = node('div', undefined, 'panel-head'); head.append(node('h2', title));
    if (note) head.append(note instanceof Node ? note : node('span', note, 'note'));
    section.append(head);
  }
  section.append(...children);
  return section;
}
function tile(label, value, detail) {
  const article = node('article', undefined, 'panel tile'), figure = node('p', undefined, 'tile-value');
  figure.append(value); article.append(node('p', label, 'tile-label'), figure);
  if (detail !== undefined) { const line = node('p', undefined, 'tile-detail'); line.append(detail); article.append(line); }
  return article;
}
// A definition list; an entry is [label, value, mono], and one without a value is left out.
function facts(entries) {
  const list = node('dl', undefined, 'facts');
  for (const [label, value, mono] of entries) {
    if (value === undefined || value === null) continue;
    const detail = node('dd', undefined, mono ? 'mono' : undefined);
    detail.append(value instanceof Node ? value : String(value)); list.append(node('dt', label), detail);
  }
  return list;
}
const group = (cls, ...children) => { const element = node('div', undefined, cls); element.append(...children); return element; };
const inline = (cls, ...children) => { const element = node('span', undefined, cls); element.append(...children); return element; };
const muted = text => node('span', text, 'muted');
// A button that copies a value, and says so.
function copyButton(text) {
  const button = node('button', undefined, 'copy'), icon = () => {
    const image = svg('svg', {viewBox: '0 0 16 16', 'aria-hidden': 'true'});
    image.append(svg('rect', {x: 5, y: 5, width: 8.5, height: 8.5, rx: 1.5}), svg('path', {d: 'M11 5V3.5A1.5 1.5 0 0 0 9.5 2h-6A1.5 1.5 0 0 0 2 3.5v6A1.5 1.5 0 0 0 3.5 11H5'}));
    return image;
  };
  button.type = 'button'; button.title = 'Copy'; button.setAttribute('aria-label', 'Copy ' + text); button.append(icon());
  button.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(text); } catch {
      const field = node('textarea'); field.value = text; document.body.append(field); field.select();
      document.execCommand('copy'); field.remove();
    }
    button.classList.add('done'); button.replaceChildren('✓');
    setTimeout(() => { button.classList.remove('done'); button.replaceChildren(icon()); }, 1200);
  });
  return button;
}
const hashCell = (hash, kind, text) => inline('hash-cell', hashLink(hash, kind, text), copyButton(hash));
const fullHash = (hash, kind) => inline('full-hash', kind ? link(hash, '/' + kind + '/' + encodeURIComponent(hash)) : hash, copyButton(hash));
function meter(percent, {alt, tick, mini} = {}) {
  const bar = node('span', undefined, 'meter' + (mini ? ' mini' : '') + (percent ? '' : ' zero'));
  const fill = node('span', undefined, 'fill' + (alt ? ' alt' : '')); fill.style.width = Math.min(100, percent || 0) + '%'; bar.append(fill);
  if (tick !== undefined) { const mark = node('span', undefined, 'tick'); mark.style.left = tick + '%'; bar.append(mark); }
  return bar;
}
function meterRow(name, percent, value, options) {
  const row = node('div', undefined, 'meter-row'); row.append(node('span', name, 'name'), meter(percent, options), node('span', value, 'val'));
  return row;
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
// A chart's numbers as a table, built when opened.
function dataTable(headers, rows) {
  const box = node('details', undefined, 'table-view'); box.append(node('summary', 'Show the data as a table'));
  box.addEventListener('toggle', () => { if (box.open && box.children.length === 1) box.append(table(headers, rows())); });
  return box;
}
function raw(label, value) {
  const disclosure = node('details'); disclosure.append(node('summary', label));
  disclosure.addEventListener('toggle', () => {
    if (disclosure.open && disclosure.children.length === 1) disclosure.append(node('pre', value === null ? 'Larger than 1 MiB: read it from its block in slices.' : typeof value === 'string' ? value : JSON.stringify(value, null, 2)));
  });
  return disclosure;
}
function empty(text) { return node('p', text, 'empty'); }
function errorView(error) { const box = node('div', undefined, 'error'); box.append(node('h2', 'Unable to load this view'), node('p', error.message), link('Return to the overview', '/')); return box; }
function staleNote(container, observed, status) {
  if (observed !== status.tip) container.append(node('p', 'The chain changed while this page loaded. Refresh to follow the latest canonical branch.', 'callout'));
}
function button(text, href) {
  if (href) return link(text, href, 'btn');
  const disabled = node('span', text, 'btn'); disabled.setAttribute('aria-disabled', 'true'); return disabled;
}

// ---- Charts ----------------------------------------------------------------

// Charts draw to their container's width, and again when it changes.
const drawers = new WeakMap(), widths = new WeakMap();
const resized = new ResizeObserver(entries => {
  for (const entry of entries) {
    const width = Math.floor(entry.contentRect.width);
    if (width && widths.get(entry.target) !== width) { widths.set(entry.target, width); drawers.get(entry.target)(width); }
  }
});
const tooltip = byId('tooltip');
function showTip(event, title, value) {
  tooltip.replaceChildren(node('div', title, 'tt-title'), node('strong', value)); tooltip.hidden = false;
  const box = tooltip.getBoundingClientRect();
  let x = event.clientX + 14, y = event.clientY + 14;
  if (x + box.width > innerWidth - 8) x = event.clientX - box.width - 14;
  if (y + box.height > innerHeight - 8) y = event.clientY - box.height - 14;
  tooltip.style.left = x + 'px'; tooltip.style.top = y + 'px';
}
const hideTip = () => { tooltip.hidden = true; };
const DURATION_STEPS = [1, 2, 5, 10, 15, 20, 30, 60, 120, 300, 600, 900, 1200, 1800, 3600, 7200, 10800, 21600, 43200, 86400];
const durationTick = s => s === 0 ? '0' : s < 60 ? s + 's' : s < 3600 ? (s / 60) + 'm' : s < 86400 ? (s / 3600) + 'h' : (s / 86400) + 'd';
// An axis reaching the peak in at most four steps of 1, 2, 2.5 or 5 times a
// power of ten, of whole numbers for counts, or of the given steps.
function axis(peak, {steps, integer} = {}) {
  let candidates = steps;
  if (!candidates) {
    const power = Math.floor(Math.log10(Math.max(peak, 1e-6) / 4));
    candidates = [];
    for (let exponent = power - 1; exponent <= power + 2; exponent++) for (const base of [1, 2, 2.5, 5]) candidates.push(base * 10 ** exponent);
    if (integer) candidates = candidates.filter(step => step >= 1 && Number.isInteger(step));
  }
  for (const step of candidates) if (Math.ceil(peak / step) <= 4) return {step, max: Math.max(1, Math.ceil(peak / step)) * step};
  // Past the given steps, whole multiples of the largest.
  const largest = candidates[candidates.length - 1], scaled = axis(peak / largest, {integer: true});
  return {step: scaled.step * largest, max: scaled.max * largest};
}
const trim = value => String(Number(value.toPrecision(6)));
// A chart of one value per block: points are {x, y, href}; kind is 'line' or
// 'bar'; a reference line can mark an average.
function chart(points, {label = '', kind = 'line', height = 180, format = String, tick = trim, steps, integer, minimum = 0, reference, referenceLabel, xLabel = x => count(x)}) {
  const container = node('div', undefined, 'chart');
  container.setAttribute('role', 'img'); container.setAttribute('aria-label', label);
  drawers.set(container, width => {
    const pad = {top: 8, right: reference !== undefined ? 58 : 6, bottom: 22, left: 40};
    const plotWidth = Math.max(10, width - pad.left - pad.right), plotHeight = height - pad.top - pad.bottom;
    const values = points.map(point => Math.max(0, point.y));
    const {step, max} = axis(Math.max(minimum, ...values, reference || 0), {steps, integer});
    const y = value => pad.top + plotHeight - value / max * plotHeight;
    const band = plotWidth / Math.max(1, points.length);
    const x = index => kind === 'bar' ? pad.left + (index + 0.5) * band : pad.left + (points.length === 1 ? plotWidth / 2 : index / (points.length - 1) * plotWidth);
    const image = svg('svg', {width, height, viewBox: '0 0 ' + width + ' ' + height});
    for (let value = 0; value <= max + step / 1e6; value += step) {
      image.append(svg('line', {x1: pad.left, x2: pad.left + plotWidth, y1: Math.round(y(value)) + 0.5, y2: Math.round(y(value)) + 0.5}, value === 0 ? 'baseline' : 'gridline'));
      const label = svg('text', {x: pad.left - 8, y: y(value) + 4, 'text-anchor': 'end'}, 'tick-label'); label.textContent = tick(value); image.append(label);
    }
    if (points.length) {
      const marks = [[0, 'start'], [points.length - 1, 'end']];
      if (points.length > 4) marks.splice(1, 0, [Math.floor((points.length - 1) / 2), 'middle']);
      for (const [index, anchor] of marks) {
        const label = svg('text', {x: kind === 'bar' ? x(index) : x(index), y: height - 4, 'text-anchor': anchor}, 'tick-label');
        label.textContent = xLabel(points[index].x); image.append(label);
      }
    }
    const bars = [];
    if (kind === 'bar') {
      const barWidth = band >= 4 ? band * 0.72 : Math.max(1, band - 0.5);
      points.forEach((point, index) => {
        const value = Math.max(0, point.y), top = y(value), heightOf = value > 0 ? Math.max(1, y(0) - top) : 0;
        const bar = svg('rect', {x: x(index) - barWidth / 2, y: y(0) - heightOf, width: barWidth, height: heightOf, rx: barWidth > 3 ? 1.5 : 0}, 'bar');
        bars.push(bar); image.append(bar);
      });
    } else if (points.length) {
      const path = points.map((point, index) => (index ? 'L' : 'M') + x(index).toFixed(1) + ' ' + y(Math.max(0, point.y)).toFixed(1)).join('');
      image.append(svg('path', {d: path + 'L' + x(points.length - 1).toFixed(1) + ' ' + y(0) + 'L' + x(0).toFixed(1) + ' ' + y(0) + 'Z'}, 'area'), svg('path', {d: path}, 'line'));
      image.append(svg('circle', {cx: x(points.length - 1), cy: y(Math.max(0, points[points.length - 1].y)), r: 3.5}, 'dot'));
    }
    if (reference !== undefined) {
      image.append(svg('line', {x1: pad.left, x2: pad.left + plotWidth, y1: Math.round(y(reference)) + 0.5, y2: Math.round(y(reference)) + 0.5}, 'ref'));
      const label = svg('text', {x: pad.left + plotWidth + 6, y: y(reference) + 4}, 'direct-label'); label.textContent = referenceLabel; image.append(label);
    }
    // Hovering shows the nearest block; clicking opens it.
    const crosshair = svg('line', {y1: pad.top, y2: pad.top + plotHeight, visibility: 'hidden'}, 'crosshair');
    const dot = svg('circle', {r: 4, visibility: 'hidden'}, 'dot');
    const hit = svg('rect', {x: pad.left, y: pad.top, width: plotWidth, height: plotHeight}, 'hit');
    let shown = -1;
    const nearest = event => {
      const box = image.getBoundingClientRect(), offset = event.clientX - box.left - pad.left;
      return Math.max(0, Math.min(points.length - 1, kind === 'bar' ? Math.floor(offset / band) : Math.round(offset / plotWidth * (points.length - 1))));
    };
    hit.addEventListener('pointermove', event => {
      if (!points.length) return;
      const index = nearest(event), point = points[index];
      if (index !== shown) {
        if (bars[shown]) bars[shown].classList.remove('hover');
        shown = index;
        if (kind === 'bar') bars[index].classList.add('hover');
        else {
          crosshair.setAttribute('x1', x(index)); crosshair.setAttribute('x2', x(index)); crosshair.setAttribute('visibility', 'visible');
          dot.setAttribute('cx', x(index)); dot.setAttribute('cy', y(Math.max(0, point.y))); dot.setAttribute('visibility', 'visible');
        }
      }
      hit.style.cursor = point.href ? 'pointer' : 'crosshair';
      showTip(event, 'Block ' + count(point.x), format(point.y));
    });
    hit.addEventListener('pointerleave', () => {
      if (bars[shown]) bars[shown].classList.remove('hover');
      shown = -1; crosshair.setAttribute('visibility', 'hidden'); dot.setAttribute('visibility', 'hidden'); hideTip();
    });
    hit.addEventListener('click', event => { const point = points[nearest(event)]; if (point && point.href) location.href = point.href; });
    image.append(crosshair, dot, hit);
    container.replaceChildren(image);
  });
  resized.observe(container);
  return container;
}

// ---- Data ----------------------------------------------------------------

// The last `n` canonical blocks, oldest first; a page the node cuts short for
// its size is continued from where it stopped.
async function recent(status, n) {
  const blocks = [];
  let from = Math.max(1, status.height - n + 1);
  while (from <= status.height) {
    const page = await json('/v1/blocks?from=' + from + '&limit=' + Math.min(100, status.height - from + 1));
    if (!page.items.length) break;
    blocks.push(...page.items); from = page.next;
  }
  return blocks;
}
// Each block's share of each resource's capacity.
const shares = blocks => RESOURCES.map((_, index) => blocks.map(block => Array.isArray(block.limits) ? share(block.used[index], block.limits[index]) || 0 : 0));

// ---- Views ---------------------------------------------------------------

async function overviewView(status) {
  setHead({title: 'Overview', sub: [network(status), ' · ', status.name || 'Ting node']});
  const blocks = status.height > 0 ? await recent(status, 100) : [];
  const container = node('div', undefined, 'stack'), tip = blocks[blocks.length - 1];
  const span = blocks.length > 1 ? tip.timestamp - blocks[0].timestamp : 0;
  const average = blocks.length > 1 ? span / (blocks.length - 1) : null;
  const transactions = blocks.reduce((sum, block) => sum + block.transactions.length, 0);
  const work = blocks.length > 1 ? BigInt(tip.work) - BigInt(blocks[0].work) : 0n;

  const hero = node('article', undefined, 'panel hero-tile'), figure = node('div');
  figure.append(node('p', 'Block height', 'kicker'), node('p', count(status.height), 'hero-value'));
  const meta = node('div', undefined, 'hero-meta');
  if (tip) {
    meta.append(group('', 'Tip ', hashCell(tip.id, 'blocks')), group('', 'Found ', age(tip.timestamp)));
  } else meta.append(group('', 'Genesis anchor: no blocks mined yet'));
  meta.append(group('', network(status) + ' · chain work ', compactNode(status.work)));
  hero.append(figure, meta);
  const limits = Array.isArray(status.limits) ? status.limits : null;
  const tiles = node('div', undefined, 'tiles');
  tiles.append(
    tile('Average block time', average === null ? '—' : duration(average), blocks.length > 1 ? 'over the last ' + (blocks.length - 1) + ' blocks' : 'needs two blocks'),
    tile('Transactions', count(transactions), blocks.length ? (transactions / blocks.length).toFixed(2) + ' per block, last ' + blocks.length : 'none yet'),
    tile('Estimated hash rate', rate(work, span), 'work per second, last ' + blocks.length + ' blocks'),
    tile('Mempool', count(status.mempool_count), size(status.mempool_bytes) + ' · ' + count(status.deferred_count) + ' deferred'),
    tile('State records', compactNode(status.state_records), count(status.state_records) + ' records'),
    tile('Block capacity', limits ? inline('', compactNode(limits[0]), node('span', 'C', 'unit')) : '—', limits ? count(limits[1]) + ' S · ' + size(limits[2]) + ' D · ' + size(limits[3]) + ' G' : undefined));
  container.append(group('hero-row', hero, tiles));
  if (!blocks.length) { container.append(panel('Latest blocks', null, empty('No blocks mined yet.'))); return container; }

  const intervals = blocks.slice(1).map((block, index) => ({x: block.height, y: block.timestamp - blocks[index].timestamp, href: '/blocks/' + block.height}));
  const perBlock = blocks.map(block => ({x: block.height, y: block.transactions.length, href: '/blocks/' + block.height}));
  container.append(group('grid two',
    panel('Block time', average === null ? null : 'average ' + duration(average),
      intervals.length ? chart(intervals, {label: 'Seconds between consecutive blocks', format: duration, tick: durationTick, steps: DURATION_STEPS, reference: average, referenceLabel: 'average'}) : empty('Needs two blocks.'),
      dataTable(['Height', {label: 'Since the previous block', cls: 'num'}], () => intervals.slice().reverse().map(point => [link(count(point.x), point.href), duration(point.y)]))),
    panel('Transactions per block', count(transactions) + ' in ' + blocks.length + ' blocks',
      chart(perBlock, {label: 'Transactions in each block', kind: 'bar', integer: true, minimum: 1, format: value => count(value) + (value === 1 ? ' transaction' : ' transactions')}),
      dataTable(['Height', {label: 'Transactions', cls: 'num'}], () => perBlock.slice().reverse().map(point => [link(count(point.x), point.href), count(point.y)])))));

  const multiples = node('div', undefined, 'multiples');
  shares(blocks).forEach((values, index) => {
    const mean = values.reduce((sum, value) => sum + value, 0) / values.length, peak = Math.max(...values);
    const item = node('div', undefined, 'multiple'), line = node('p', percentText(mean), 'figure');
    line.append(node('span', 'average · peak ' + percentText(peak), 'unit'));
    item.append(node('h3', RESOURCES[index][1] + ' (' + RESOURCES[index][0] + ')'), line,
      chart(values.map((value, offset) => ({x: blocks[offset].height, y: value, href: '/blocks/' + blocks[offset].height})), {label: RESOURCES[index][1] + ' used by each block, as a share of its capacity', kind: 'bar', height: 92, minimum: 0.5, format: percentText, tick: value => trim(value) + '%', xLabel: () => ''}));
    multiples.append(item);
  });
  container.append(panel('How full blocks are', 'each resource as a share of its capacity, last ' + blocks.length + ' blocks', multiples));

  container.append(panel('Latest blocks', link('All blocks →', '/blocks'), latestTable(blocks.slice(-10).reverse())));
  container.append(panel('Chain', null, facts([['Chain ID', fullHash(status.chain_id)], ['Canonical tip', fullHash(status.tip, 'blocks')], ['State root', fullHash(status.state_root)]])));
  return container;
}
function latestTable(blocks) {
  return table(['Height', 'Age', {label: 'Transactions', cls: 'num'}, {label: 'Events', cls: 'num'}, {label: 'Size', cls: 'num'}, {label: 'Fullest resource', title: 'The resource nearest its capacity in each block'}, 'Block'],
    blocks.map(block => [link(count(block.height), '/blocks/' + block.height), age(block.timestamp, 'muted'), count(block.transactions.length), count(events(block)), size(block.bytes), fullest(block), hashCell(block.id, 'blocks')]));
}
const RESOURCE_COLUMNS = RESOURCES.map(([, name]) => ({label: name === 'State access' ? 'State' : name, cls: 'num', title: name + ' used, and its share of the block’s capacity'}));
function blockTable(blocks, emptyText) {
  if (!blocks.length) return empty(emptyText);
  return table(['Height', 'Age', {label: 'Transactions', cls: 'num'}, {label: 'Events', cls: 'num'}, {label: 'Size', cls: 'num'}, ...RESOURCE_COLUMNS, 'Block'], blocks.slice().reverse().map(block => [
    link(count(block.height), '/blocks/' + block.height), age(block.timestamp, 'muted'), count(block.transactions.length), count(events(block)), size(block.bytes),
    ...RESOURCES.map((_, index) => usageCell(block.used[index], Array.isArray(block.limits) ? block.limits[index] : undefined)),
    hashCell(block.id, 'blocks')
  ]));
}
const PAGE = 25;
async function blocksView(status) {
  const selected = new URLSearchParams(location.search).get('from');
  const from = selected === null ? Math.max(1, status.height - PAGE + 1) : Number(selected);
  if (!Number.isSafeInteger(from) || from < 1) throw Error('Block pagination starts at a positive height.');
  const page = await json('/v1/blocks?from=' + from + '&limit=' + PAGE);
  const last = Math.max(from, page.next - 1);
  setHead({title: 'Blocks', sub: page.items.length ? 'Heights ' + count(from) + '–' + count(last) + ' of ' + count(status.height) : 'No blocks in this range'});
  const pagination = node('nav', undefined, 'pagination');
  pagination.setAttribute('aria-label', 'Block pages');
  pagination.append(button('← Older', from > 1 ? '/blocks?from=' + Math.max(1, from - PAGE) : undefined),
    page.next <= status.height ? group('actions', button('Newer →', '/blocks?from=' + page.next), button('Latest', '/blocks')) : muted('This is the tip'));
  const container = node('div', undefined, 'stack');
  container.append(panel(null, null, blockTable(page.items, 'No mined blocks in this range.'), pagination));
  staleNote(container, page.tip, status);
  return container;
}
const VOTES = ['hold', 'increase', 'decrease'];
function votes(values) {
  const list = node('span', undefined, 'chips');
  RESOURCES.forEach(([symbol, name], index) => {
    const chip = node('span', symbol + ' ' + ({0: '–', 1: '↑', 2: '↓'}[values[index]] ?? values[index]), 'chip');
    chip.title = name + ': ' + (VOTES[values[index]] || 'unknown'); list.append(chip);
  });
  return list;
}
async function blockView(id, status) {
  const block = await json('/v1/blocks/' + encodeURIComponent(id));
  const [parent, voting] = await Promise.all([
    block.height > 1 ? json('/v1/blocks/' + block.parent).catch(() => null) : null,
    json('/v1/votes').catch(() => null)]);
  const confirmations = block.canonical ? status.height - block.height + 1 : 0;
  const badges = [block.canonical ? pill('Canonical', 'success') : pill('Side branch', 'warn')];
  if (block.canonical) badges.push(pill(count(confirmations) + (confirmations === 1 ? ' confirmation' : ' confirmations'), 'plain'));
  setHead({kicker: 'Block', title: count(block.height), badges, sub: [timestamp(block.timestamp) + ' UTC · ', age(block.timestamp)], actions: [
    button('← Previous', block.height > 1 ? '/blocks/' + (block.canonical ? block.height - 1 : block.parent) : undefined),
    button('Next →', block.canonical && block.height < status.height ? '/blocks/' + (block.height + 1) : undefined)]});
  const work = parent ? BigInt(block.work) - BigInt(parent.work) : null;
  const summary = panel('Summary', null, facts([
    ['Block id', fullHash(block.id)],
    ['Parent', block.height === 1 ? 'Genesis anchor' : hashCell(block.parent, 'blocks')],
    ['Time', inline('', timestamp(block.timestamp) + ' UTC ', muted('· '), age(block.timestamp, 'muted'))],
    ['Transactions', count(block.transactions.length)],
    ['Events', count(events(block))],
    ['Size', size(block.bytes) + (block.bytes >= 1024 ? ' (' + count(block.bytes) + ' bytes)' : '')],
    ['Producer data', block.producer_hex ? size(block.producer_hex.length / 2) : 'none'],
    ['Capacity votes', votes(block.votes)],
    ['Block work', work === null ? undefined : compactNode(work)],
    ['Chain work', compactNode(block.work)],
    ['Target', block.target, true],
    ['Nonce', block.nonce, true],
    ['State root', fullHash(block.state_root)],
    ['Transactions root', fullHash(block.transactions_root)],
    ['Receipts root', fullHash(block.receipts_root)]
  ]), group('actions', link('Raw block', '/v1/blocks/' + encodeURIComponent(block.id) + '/raw', 'btn'), link('Block JSON', '/v1/blocks/' + encodeURIComponent(block.id), 'btn')));
  const used = node('div', undefined, 'meters');
  RESOURCES.forEach(([symbol, name], index) => {
    const limit = Array.isArray(block.limits) ? block.limits[index] : undefined, percent = share(block.used[index], limit);
    const row = meterRow(name + ' (' + symbol + ')', percent || 0, amount(index, block.used[index]) + (limit === undefined ? '' : ' / ' + amount(index, limit) + ' · ' + percentText(percent)));
    row.title = count(block.used[index]) + (limit === undefined ? '' : ' of ' + count(limit)); used.append(row);
  });
  const epochs = voting && voting.epoch_blocks, epoch = epochs ? Math.floor((block.height - 1) / epochs) : null;
  const timing = panel('Timing', null, facts([
    ['Since the previous block', parent ? duration(block.timestamp - parent.timestamp) : undefined],
    ['Capacity epoch', epochs ? 'epoch ' + epoch + ', block ' + count(block.height - epoch * epochs) + ' of ' + count(epochs) : undefined],
    ['Work share', work === null || BigInt(block.work) === 0n ? undefined : percentText(Number(work * 1000000n / BigInt(block.work)) / 10000) + ' of the chain’s work']
  ]));
  const container = node('div', undefined, 'stack');
  container.append(group('grid split', summary, group('stack', panel('Resources used', 'of this block’s capacity', used), timing)));
  const transactions = block.transactions.length ? table(['#', 'Transaction', 'Outcome', {label: 'Used C / S / D / G', cls: 'num'}, {label: 'Events', cls: 'num'}, {label: 'Result', cls: 'num'}], block.transactions.slice(0, 200).map((tx, index) => {
    const receipt = block.receipts[index];
    return [count(index), hashCell(tx, 'transactions'), badge(receipt), node('span', resources(receipt.used), 'mono'), count(receipt.events.length), size(receipt.result_hex.length / 2)];
  })) : empty('No transactions.');
  container.append(panel('Transactions', count(block.transactions.length), transactions,
    ...(block.transactions.length > 200 ? [node('p', 'Showing the first 200 transactions. The API provides the complete block data.', 'muted')] : [])));
  container.append(eventsView(block.receipts), panel('Raw data', null, group('raw-data', raw('Producer data (hex)', block.producer_hex), raw('Complete block response', block))));
  return container;
}
function eventsView(receipts) {
  const list = receipts.flatMap((receipt, index) => receipt.events.map(event => ({...event, transaction: receipt.transaction, index})));
  const content = list.length ? table(['Transaction', 'Contract', 'Payload (hex)'], list.slice(0, 200).map(event => [
    hashCell(event.transaction, 'transactions'), hashCell(event.contract, 'contracts'), raw(event.payload_hex ? short(event.payload_hex) : 'Empty payload', event.payload_hex)
  ])) : empty('No events.');
  return panel('Events', count(list.length), content, ...(list.length > 200 ? [node('p', 'Showing the first 200 events. The API provides the complete block data.', 'muted')] : []));
}
function envelope(summary) {
  if (!summary) return node('p', 'Envelope summary unavailable.', 'muted');
  return facts([
    ['Kind', summary.kind], ['Budget C / S / D / G', resources(summary.budget), true], ['Envelope', size(summary.encoded_bytes)], ['Input', size(summary.input_bytes)],
    ['Target contract', summary.target ? hashCell(summary.target, 'contracts') : undefined],
    ['Original code', summary.code_bytes === null ? undefined : size(summary.code_bytes)]
  ]);
}
async function transactionView(id, status) {
  const offset = new URLSearchParams(location.search).get('from') || '0';
  const tx = await json('/v1/transactions/' + encodeURIComponent(id) + '?from=' + encodeURIComponent(offset) + '&limit=20');
  watchPending = tx.pending;
  const badges = tx.pending ? [pill('Pending', 'warn')] : tx.occurrences.length ? [pill('Included', 'success')] : [];
  setHead({kicker: 'Transaction', title: short(tx.id), mono: true, badges});
  const container = node('div', undefined, 'stack');
  container.append(panel('Summary', null, facts([
    ['Transaction ID', fullHash(tx.id)],
    ['Status', tx.pending ? 'In this node’s pool, awaiting a block' : tx.occurrences.length ? 'In the canonical chain' : 'Not in the canonical chain'],
    ['Occurrences', count(tx.occurrences.length) + (tx.has_more ? '+' : '')]
  ])));
  if (tx.pending_transaction) container.append(panel('Pending envelope', null, envelope(tx.pending_transaction.transaction), raw('Pending canonical envelope (hex)', tx.pending_transaction.hex)));
  for (const occurrence of tx.occurrences) {
    const receipt = occurrence.receipt, parts = [facts([
      ['Block', inline('', link(count(occurrence.height), '/blocks/' + occurrence.height), ' ', hashCell(occurrence.block, 'blocks'))],
      ['Outcome', badge(receipt)], ['Used C / S / D / G', resources(receipt.used), true], ['Result', size(receipt.result_hex.length / 2)],
      ['Deployed contract', receipt.status === 0 && occurrence.transaction && occurrence.transaction.kind === 'deploy' && receipt.result_hex.length === 64 ? hashCell(receipt.result_hex, 'contracts') : undefined]
    ])];
    parts.push(node('h3', 'Envelope'), envelope(occurrence.transaction), group('raw-data', raw('Result (hex)', receipt.result_hex), raw('Canonical envelope (hex)', occurrence.hex)));
    container.append(panel('Height ' + count(occurrence.height) + ' · transaction ' + count(occurrence.index), null, ...parts), eventsView([receipt]));
  }
  if (!tx.occurrences.length && !tx.pending_transaction) container.append(panel(null, null, empty('Awaiting block inclusion.')));
  if (tx.has_more) container.append(group('actions', link('More occurrences →', '/transactions/' + encodeURIComponent(id) + '?from=' + tx.next, 'btn')));
  staleNote(container, tx.tip, status);
  return container;
}
async function poolView(status) {
  setHead({title: 'Mempool', sub: 'Transactions this node holds for its next blocks'});
  const pool = await json('/v1/mempool'), container = node('div', undefined, 'stack');
  container.append(group('tiles', tile('Pending transactions', count(pool.items.length)), tile('Pending bytes', size(pool.bytes), count(pool.bytes) + ' bytes'),
    tile('Deferred', count(status.deferred_count), 'held until their not-before time')));
  container.append(panel('Pending transactions', null, pool.items.length ? table(['Transaction', {label: 'Envelope', cls: 'num'}, {label: 'Budget C / S / D / G', cls: 'num'}],
    pool.items.map(tx => [hashCell(tx.id, 'transactions'), size(tx.bytes), node('span', resources(tx.budget), 'mono')])) : empty('The transaction pool is empty.')));
  return container;
}
async function contractView(id) {
  const contract = await json('/v1/contracts/' + encodeURIComponent(id)), container = node('div', undefined, 'stack');
  setHead({kicker: 'Contract', title: short(contract.id), mono: true});
  container.append(panel('Code', null, facts([['Contract ID', fullHash(contract.id)], ['Original code hash', fullHash(contract.code_hash)], ['Code', size(contract.code_bytes)],
    ['Code price', contract.code_price === undefined ? undefined : count(contract.code_price) + ' C'], ['Current state root', fullHash(contract.state_root)]])));
  const form = node('form', undefined, 'storage-form'), input = node('input'), submit = node('button', 'Read record');
  input.id = 'storage-key'; input.placeholder = '32-byte storage key (64 hex characters)'; input.required = true; input.pattern = '[0-9a-fA-F]{64}'; input.spellcheck = false; input.autocomplete = 'off'; input.setAttribute('aria-label', 'Storage key');
  submit.type = 'submit'; form.append(input, submit); const result = node('div');
  form.addEventListener('submit', async event => {
    event.preventDefault(); submit.disabled = true;
    try {
      const value = await json('/v1/contracts/' + encodeURIComponent(id) + '/storage/' + input.value.trim().toLowerCase());
      result.replaceChildren(group('result-box', facts([['Record', value.value_hex === null ? 'Absent' : value.value_hex === '' ? 'Present, empty value' : 'Present'], ['Value (hex)', value.value_hex === null ? '—' : value.value_hex, true], ['State root', value.state_root, true]])));
    } catch (error) { result.replaceChildren(errorView(error)); } finally { submit.disabled = false; }
  });
  container.append(panel('Storage', 'one record by its key', form, result));
  return container;
}
async function networkView(status) {
  setHead({title: 'Network', sub: 'Capacity voting, the capacities it decides, and this node'});
  const [voting, start] = await Promise.all([json('/v1/votes'), status.height > 1 ? json('/v1/blocks/' + Math.max(1, status.height - 99)) : null]);
  const tipTime = status.height > 1 ? (await json('/v1/blocks/' + status.height)).timestamp : null;
  const average = start && status.height > start.height ? (tipTime - start.timestamp) / (status.height - start.height) : null;
  const epoch = voting.this_epoch, remaining = voting.next_epoch_from - voting.height;
  const progress = epoch.blocks / voting.epoch_blocks * 100;
  const epochPanel = panel('Capacity epoch', 'epoch ' + count(voting.epoch), group('meters', meterRow('Progress', progress, count(epoch.blocks) + ' of ' + count(voting.epoch_blocks) + ' blocks')), facts([
    ['Next epoch', 'from height ' + count(voting.next_epoch_from) + ', in ' + count(remaining) + ' blocks' + (average ? ' (about ' + duration(remaining * average) + ')' : '')],
    ['This epoch’s votes apply', 'from height ' + count(epoch.takes_effect_from)]
  ]));
  epochPanel.querySelector('.facts').style.marginTop = '16px';
  const capacities = panel('Capacities', 'decided three epochs ahead', table(['Epoch', {label: 'From height', cls: 'num'}, ...RESOURCES.map(([symbol, name]) => ({label: symbol, cls: 'num', title: name}))],
    voting.capacities.map(item => [count(item.epoch), count(item.from), compactNode(item.limits[0]), count(item.limits[1]), size(item.limits[2]), size(item.limits[3])])));
  const warnings = Array.isArray(status.warnings) && status.warnings.length ? status.warnings.join('; ') : undefined;
  const self = panel('This node', 'v' + status.version, facts([
    ['Name', status.name], ['Network', network(status)], ['Chain ID', fullHash(status.chain_id)], ['Genesis block', fullHash(status.genesis)],
    ['Up for', duration(status.uptime_seconds)], ['Canonical height', count(status.height)], ['Best header', count(status.best_height)],
    ['Headers held', count(status.held_headers)], ['Stored blocks', count(status.stored_blocks)], ['Settled height', count(status.settled_height)],
    ['State records', count(status.state_records)], ['Replaying', status.replaying ? 'yes' : undefined],
    ['Reorganization', status.reorganization ? JSON.stringify(status.reorganization) : undefined], ['Warnings', warnings]
  ]));
  const legend = node('div', undefined, 'legend');
  legend.append(group('', node('span', undefined, 'key'), 'Increase · needs two-thirds'), group('', node('span', undefined, 'key alt'), 'Decrease · needs more than half'));
  const tally = panel('Votes this epoch', 'share of the epoch’s work voting each way', legend);
  RESOURCES.forEach(([symbol, name], index) => {
    const outcome = epoch.outcome_so_far[index], head = node('div', undefined, 'vote-head');
    head.append(node('h3', name + ' (' + symbol + ')'), pill('so far: ' + outcome, outcome === 'increase' ? 'accent' : outcome === 'decrease' ? 'warn' : undefined));
    const up = epoch.increase_percent[index], down = epoch.decrease_percent[index];
    tally.append(group('vote-group', head, group('meters',
      meterRow('Increase', up, percentText(up) + ' of the work', {tick: 200 / 3}),
      meterRow('Decrease', down, percentText(down) + ' of the work', {alt: true, tick: 50}))));
  });
  return group('grid two', group('stack', epochPanel, capacities, self), tally);
}
function render(status) {
  if (kind === 'overview') return overviewView(status);
  if (kind === 'blocks' && target === undefined) return blocksView(status);
  if (kind === 'blocks') return blockView(target, status);
  if (kind === 'transactions') return transactionView(target, status);
  if (kind === 'contracts') return contractView(target);
  if (kind === 'mempool') return poolView(status);
  if (kind === 'network') return networkView(status);
  throw Error('Unknown explorer page.');
}

// ---- Following the node ------------------------------------------------------

// The overview, the latest blocks, the pool and the network follow the tip;
// other pages stay as they were loaded, but their ages keep counting.
const FOLLOW = new Set(['overview', 'mempool', 'network']);
const follows = () => FOLLOW.has(kind) || (kind === 'blocks' && target === undefined && !location.search) || watchPending;
let busy = false, watchPending = false, rendered = '';
async function update(status) {
  const pending = byId('nav-pending');
  pending.hidden = !status.mempool_count; pending.textContent = count(status.mempool_count);
  document.querySelectorAll('[data-age]').forEach(element => { element.textContent = ago(element.dataset.age); });
  const observed = status.tip + ' ' + status.mempool_count;
  if (busy || (rendered && (!follows() || observed === rendered))) return;
  busy = true;
  try {
    // The charts being replaced stop following their size.
    resized.disconnect();
    let content;
    try { content = await render(status); } catch (error) { content = errorView(error); }
    hideTip();
    byId('view').replaceChildren(content); rendered = observed;
  } finally { busy = false; }
}
setHead({kicker: kind === 'blocks' && target !== undefined ? 'Block' : kind === 'transactions' ? 'Transaction' : kind === 'contracts' ? 'Contract' : '',
  title: kind === 'blocks' && target !== undefined ? (/^[0-9]+$/.test(target) ? count(target) : short(target)) :
    ({overview: 'Overview', blocks: 'Blocks', transactions: short(target || ''), contracts: short(target || ''), mempool: 'Mempool', network: 'Network'})[kind] || 'Not found'});
document.querySelectorAll('[data-view]').forEach(a => { if (a.dataset.view === kind) a.setAttribute('aria-current', 'page'); });
document.addEventListener('ting:status', event => update(event.detail));
document.addEventListener('ting:offline', event => { if (!rendered) byId('view').replaceChildren(errorView(event.detail)); });
// The header may have read the status before this script ran.
if (window.tingStatus) update(window.tingStatus);
