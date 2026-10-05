// SPDX-License-Identifier: MIT
'use strict';
function parseChild(detail, side) {
  const marker = side + ': ';
  const start = detail.indexOf(marker);
  if (start < 0) return null;
  const end = side === 'tx' ? detail.indexOf(' rx: ', start + marker.length) : -1;
  const text = detail.slice(start + marker.length, end < 0 ? undefined : end);
  for (const line of text.split('\n')) {
    try {
      const record = JSON.parse(line.trim());
      if (record && typeof record === 'object' && typeof record.status === 'string') return record;
    } catch (_) { /* Operational stderr is shown in the full case details. */ }
  }
  return null;
}
function reportRows(log) {
  const rows = log.split(/\r?\n/).filter(line => line.trim()).map((line, index) => {
    try {
      const record = JSON.parse(line);
      const statuses = ['PASS', 'FAIL', 'SKIP', 'OBSERVED', 'INFO', 'PLAN'];
      if (!record || !statuses.includes(record.status) || typeof record.case !== 'string') throw new Error('Invalid record schema');
      const detail = typeof record.detail === 'string' ? record.detail : '';
      const tx = parseChild(detail, 'tx'), rx = parseChild(detail, 'rx');
      const measured = rx || record;
      const errors = [...(Array.isArray(tx?.errors) ? tx.errors : []), ...(Array.isArray(measured.errors) ? measured.errors : [])];
      return {record, measured, errors, status: record.status, name: record.case, detail};
    } catch (error) {
      return {record: {}, measured: {}, errors: [String(error)], status: 'INVALID', name: 'Log line ' + (index + 1), detail: line};
    }
  });
  const inventory = new Map();
  for (const row of rows) {
    const match = row.detail.match(/^driver=(.*?) card=(.*?) bus=(.*?) direction=/);
    if (!match) continue;
    const refs = endpointRefs(row.name);
    if (refs.length !== 1) continue;
    const ref = refs[0];
    inventory.set((ref.agent || '') + '|' + ref.device,
      {name: match[2], driver: match[1], bus: match[3], device: ref.device, agent: ref.agent});
  }
  for (const row of rows) {
    const explicit = Array.isArray(row.record.boards) ? row.record.boards : [];
    const refs = endpointRefs(row.name);
    row.boards = refs.length ? refs.flatMap(ref => {
      const captured = explicit.filter(board => board.device === ref.device && (!board.agent || !ref.agent || board.agent === ref.agent));
      if (captured.length === 1) return captured;
      const exact = inventory.get((ref.agent || '') + '|' + ref.device);
      if (exact) return [exact];
      const candidates = [...inventory.values()].filter(board => board.device === ref.device);
      // Old single-host logs have no agent field. Never guess between different servers.
      if (candidates.length === 1 && (!candidates[0].agent || !ref.agent)) return candidates;
      return [];
    }) : explicit;
    row.boardLabel = row.boards.map(board => board.name).join(' → ');
  }
  return rows;
}
function endpointRefs(name) {
  return [...name.matchAll(/(?:(http:\/\/[^\s]+|local)\s+)?(\/dev\/video\d+)\b/g)]
    .map(match => ({agent: match[1] || null, device: match[2]}));
}
function reportCounts(rows) {
  const counts = {PASS: 0, FAIL: 0, SKIP: 0, OBSERVED: 0, INVALID: 0};
  for (const row of rows) if (Object.hasOwn(counts, row.status)) counts[row.status]++;
  return counts;
}
function renderReport() {
  const rows = reportRows(document.getElementById('log').textContent);
  const counts = reportCounts(rows);
  const connections = rows.filter(row => row.name.startsWith('connection '));
  const connectionSection = document.getElementById('connections');
  connectionSection.hidden = connections.length === 0;
  const connectionList = document.getElementById('connection-list');
  connectionList.replaceChildren();
  for (const link of connections) {
    const card = document.createElement('article'); card.className = 'connection';
    const name = document.createElement('strong'); name.textContent = link.boardLabel || link.name.slice('connection '.length);
    const proof = document.createElement('p'); proof.textContent = link.name.slice('connection '.length) + '\nVideo probe confirmed. Feature results are listed below.';
    card.append(name, proof); connectionList.appendChild(card);
  }
  for (const [status, count] of Object.entries(counts)) {
    const card = document.createElement('div'); card.className = 'count ' + status.toLowerCase();
    card.textContent = status + ': ' + count; document.getElementById('summary').appendChild(card);
  }
  document.getElementById('coverage').textContent = counts.PASS + counts.FAIL === 0
    ? 'No completed validation cases. Observations and skipped cases do not prove full coverage.'
    : 'Completed validation cases: ' + (counts.PASS + counts.FAIL) + '. Each row preserves the original result.';
  const filter = document.getElementById('status'), search = document.getElementById('search');
  function cell(row, value) { const td = document.createElement('td'); td.textContent = String(value); row.appendChild(td); return td; }
  function draw() {
    const body = document.getElementById('cases'); body.replaceChildren();
    const query = search.value.toLowerCase();
    for (const item of rows) {
      if (filter.value && item.status !== filter.value) continue;
      if (query && !(item.boardLabel + ' ' + item.name + ' ' + item.detail + ' ' + item.errors.join(' ')).toLowerCase().includes(query)) continue;
      const row = document.createElement('tr'), data = item.measured;
      cell(row, item.boardLabel || '—'); cell(row, item.name); cell(row, item.status).className = item.status.toLowerCase();
      cell(row, (data.frames ?? '—') + ' / ' + (data.gaps ?? '—'));
      cell(row, data.crc_errors ?? '—'); cell(row, (data.audio_present_channels ?? '—') + ' / ' + (data.audio_nonzero_channels ?? '—'));
      const anc = data.anc && typeof data.anc === 'object' ? Object.entries(data.anc).map(([key, value]) => key + ': ' + value).join(', ') : '';
      const td = cell(row, anc + (item.errors.length ? '\n' + item.errors.join('\n') : ''));
      const details = document.createElement('details'), title = document.createElement('summary'), pre = document.createElement('pre');
      title.textContent = 'Full details'; pre.textContent = item.detail; details.append(title, pre); td.appendChild(details); body.appendChild(row);
    }
  }
  filter.addEventListener('change', draw); search.addEventListener('input', draw); draw();
}
if (typeof document !== 'undefined') renderReport();
if (typeof module !== 'undefined') module.exports = {reportRows, reportCounts};
