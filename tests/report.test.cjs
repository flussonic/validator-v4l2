// SPDX-License-Identifier: MIT
const test = require('node:test');
const assert = require('node:assert/strict');
const {reportRows, reportCounts} = require('../src/report.js');
test('paired result preserves transmitter failure and measured receiver counters', () => {
  const tx = {status:'FAIL', errors:['DMA failure']};
  const rx = {status:'PASS', frames:40, gaps:0, crc_errors:5, audio_present_channels:16, errors:[]};
  const parent = {status:'FAIL', case:'pair 1080p50', detail:'tx: '+JSON.stringify(tx)+'\n  rx: '+JSON.stringify(rx)+'\n '};
  const rows = reportRows(JSON.stringify(parent));
  assert.equal(rows[0].status, 'FAIL');
  assert.equal(rows[0].measured.crc_errors, 5);
  assert.equal(rows[0].measured.audio_present_channels, 16);
  assert.deepEqual(rows[0].errors, ['DMA failure']);
  assert.equal(reportCounts(rows).PASS, 0);
  assert.equal(reportCounts(rows).FAIL, 1);
});
test('skips, observations and broken log lines never count as passes', () => {
  const log = ['SKIP','OBSERVED','PASS'].map(status => JSON.stringify({status,case:'case'})).join('\n')+'\n{truncated\n';
  assert.deepEqual(reportCounts(reportRows(log)), {PASS:1,FAIL:0,SKIP:1,OBSERVED:1,INVALID:1});
});
