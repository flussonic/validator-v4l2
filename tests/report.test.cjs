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
test('board names follow endpoint identity across two servers with identical node paths', () => {
  const inventory = (host, card) => ({status:'INFO',case:host+' /dev/video0',detail:'driver=vendor card='+card+' bus=synthetic-bus direction=capture formats=SDUY'});
  const rows = reportRows([
    inventory('http://sender:5040', 'KONA 5 SDI out 1'),
    inventory('http://receiver:5040', 'DeckLink 8K Pro SDI in 1'),
    {status:'PASS',case:'http://sender:5040 /dev/video0 -> http://receiver:5040 /dev/video0 Case { mode: 1080p25 }',detail:''},
    {status:'SKIP',case:'http://unknown:5040 /dev/video0',detail:''}
  ].map(JSON.stringify).join('\n'));
  assert.equal(rows[2].boardLabel, 'KONA 5 SDI out 1 → DeckLink 8K Pro SDI in 1');
  assert.equal(rows[3].boardLabel, '');
});
test('old single-host reports resolve board names for discovered connections', () => {
  const rows = reportRows([
    {status:'INFO',case:'/dev/video4',detail:'driver=aja card=KONA 5 SDI out 1 bus=synthetic-bus direction=output'},
    {status:'INFO',case:'/dev/video0',detail:'driver=decklink card=DeckLink 8K Pro SDI in 1 bus=synthetic-bus direction=capture'},
    {status:'INFO',case:'connection http://localhost:5040 /dev/video4 -> http://localhost:5040 /dev/video0',detail:''}
  ].map(JSON.stringify).join('\n'));
  assert.equal(rows[2].boardLabel, 'KONA 5 SDI out 1 → DeckLink 8K Pro SDI in 1');
});
