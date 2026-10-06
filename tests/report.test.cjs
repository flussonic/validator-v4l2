// SPDX-License-Identifier: MIT
const test = require('node:test');
const assert = require('node:assert/strict');
const {reportRows, reportCounts} = require('../src/report.js');
test('4K coverage distinguishes absent advertised modes from recorded skips and measurements', () => {
  const {fourKCoverage}=require('../src/report.js');
  assert.match(fourKCoverage({rows:[{name:'1920x1080p25.000',capabilityKey:'output timings=1920x1080p25.000 -> capture timings=1280x720p50.000'}]}),/TX объявляет.*1080; RX объявляет.*720/);
  assert.match(fourKCoverage({rows:[{name:'3840x2160p50.000',status:'SKIP'},{name:'3840x2160p50.000',status:'PASS'},{name:'4096x2160p25.000',status:'FAIL'}]}),/2 сценариев.*3 отдельных результатов/);
  assert.match(fourKCoverage({rows:[{name:'1080p25'}]}),/Наличие поддержки.*установить нельзя/);
  assert.equal(fourKCoverage({rows:[{name:'Case { mode: "ASI" }'}]}),'');
});
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
const {scenarioRows,reportGroups} = require('../src/report.js');
test('checks are independent of the combined scenario and connection rows never count', () => {
  const route='http://tx:1 /dev/video0 -> http://rx:1 /dev/video2';
  const tx={status:'OBSERVED',checks:[{name:'stream continuity',status:'PASS',observations:50,errors:[]}]};
  const rx={status:'FAIL',checks:[{name:'PCM payload',status:'PASS',observations:50,errors:[]},{name:'ANC 60/60',status:'FAIL',observations:50,errors:['lost']},{name:'SD VBI',status:'SKIP',observations:0,reason:'HD'}]};
  const raw=reportRows([{status:'INFO',case:'connection '+route,detail:'probe proof'}, {status:'FAIL',case:route+' Case { mode: "1080p25", format: "SDUY" }',detail:'tx: '+JSON.stringify(tx)+'\n rx: '+JSON.stringify(rx)}].map(JSON.stringify).join('\n'));
  const grouped=reportGroups(raw);
  assert.equal(grouped.validation.length,1);
  assert.deepEqual(grouped.validation[0].counts,{PASS:2,FAIL:1,SKIP:1,OBSERVED:0,INVALID:0});
  assert.equal(grouped.validation[0].rows.length,4);
  assert.match(grouped.validation[0].rows[0].detail,/probe proof/);
});
test('same physical board pair uses one representative and compares cable scenario outcomes', () => {
  const record=(tx,rx,status)=>({status,case:`http://tx:1 /dev/video${tx} -> http://rx:1 /dev/video${rx} Case { mode: "1080p25", format: "SDUY" }`,detail:'',boards:[{name:'Sender',device:'/dev/video'+tx,bus:'PCI:1',driver:'x',agent:'http://tx:1'},{name:'Receiver',device:'/dev/video'+rx,bus:'PCI:2',driver:'y',agent:'http://rx:1'}]});
  const groups=reportGroups(reportRows([record(0,2,'PASS'),record(4,6,'PASS'),record(8,10,'FAIL')].map(JSON.stringify).join('\n')));
  assert.equal(groups.validation.length,1);assert.equal(groups.cables.length,2);
  assert.match(groups.cables[0].comparison,/identical/);assert.match(groups.cables[1].comparison,/differ/);
  assert.equal(groups.validation[0].counts.PASS,1);
});
test('untested equivalent cables do not inherit representative passes', () => {
  const route='http://tx:1 /dev/video4 -> http://rx:1 /dev/video6';
  const groups=reportGroups(reportRows([
    {status:'PASS',case:'http://tx:1 /dev/video0 -> http://rx:1 /dev/video2 Case { mode: "1080p25" }',detail:''},
    {status:'INFO',case:'equivalent route '+route,detail:JSON.stringify({representative:'http://tx:1 /dev/video0 -> http://rx:1 /dev/video2',tested:false})}
  ].map(JSON.stringify).join('\n')));
  assert.equal(groups.cables.length,1);assert.equal(groups.cables[0].rows.length,0);
  assert.equal(groups.cables[0].counts.PASS,0);assert.match(groups.cables[0].comparison,/not been tested/);
});
test('worker setup failure remains visible when the other worker has checks', () => {
  const tx={status:'OBSERVED',checks:[{name:'stream continuity',status:'PASS',observations:10}]};
  const raw=reportRows(JSON.stringify({status:'FAIL',case:'pair',detail:'tx: '+JSON.stringify(tx)+'\n rx: timeout'}));
  const rows=scenarioRows(raw);assert.equal(rows.length,2);assert.equal(rows[1].status,'FAIL');assert.match(rows[1].checkName,/RX/);
});
test('report renders grouped counts and puts probe evidence only into case details', () => {
  const vm=require('node:vm'), fs=require('node:fs');
  class Element {
    constructor(tag) {this.tag=tag;this.children=[];this.value='';this.textContent='';}
    appendChild(node){this.children.push(node);return node;}
    append(...nodes){this.children.push(...nodes);}
    replaceChildren(...nodes){this.children=nodes;}
    addEventListener(){}
  }
  const nodes=new Map(['log','status','search','summary','coverage','route-groups','cable-groups','unavailable-groups','diagnostics'].map(id=>[id,new Element('div')]));
  nodes.get('log').textContent=[
    {status:'INFO',case:'connection http://tx:1 /dev/video0 -> http://rx:1 /dev/video2',detail:'unique probe evidence'},
    {status:'PASS',case:'http://tx:1 /dev/video0 -> http://rx:1 /dev/video2 Case { mode: "1080p25" }',detail:''}
  ].map(JSON.stringify).join('\n');
  const document={getElementById:id=>{assert.ok(nodes.has(id),id);return nodes.get(id);},createElement:tag=>new Element(tag)};
  vm.runInNewContext(fs.readFileSync(require.resolve('../src/report.js'),'utf8'),{document});
  const group=nodes.get('route-groups').children[0];
  assert.match(group.children[0].textContent,/PASS 1, FAIL 0, SKIP 0/);
  const text=node=>node.textContent+'\n'+node.children.map(text).join('\n');
  assert.match(text(group),/unique probe evidence/);
  assert.equal(nodes.get('diagnostics').children.length,0);
});
const {phaseGroups,checkLabel,failureSummary}=require('../src/report.js');
test('feature phases put stereo before sixteen channels and EAC3 before ANC',()=>{
 const record=(config,check)=>({name:'Case { mode: "1080p25", format: "SDUY", config: '+config+' }',checkName:check});
 const rows=[record('channels: 2, anc: true','RX · ANC 60/60'),record('channels: 16, anc: false, vbi: false','RX · PCM channel 16'),record('channels: 2, anc: false, vbi: false','RX · video pattern'),record('channels: 2, anc: false, eac3: true','RX · encoded audio')];
 assert.deepEqual(phaseGroups(rows).map(phase=>phase.order),[1,2,3,5]);
 assert.match(checkLabel('RX · ANC 60/60',rows[0]),/Таймкод ATC/);
 assert.match(checkLabel('RX · ANC 41/07',rows[0]),/SCTE-104/);
 assert.match(failureSummary({...rows[0],status:'FAIL',errors:['ANC 60/60: expected packet absent for picture 112']}),/Таймкод ATC/);
 assert.match(failureSummary({...rows[1],status:'FAIL',errors:['PCM channel 16: 400/1920 tone samples mismatch or lose continuity']}),/Канал 16: 400 из 1920/);
});
test('disabled ancillary checks and redundant PCM aggregate do not clutter stereo results',()=>{
 const rx={status:'PASS',checks:[{name:'video pattern',status:'PASS',observations:50},{name:'PCM payload',status:'PASS',observations:50},{name:'PCM channel 1',status:'PASS',observations:50},{name:'PCM channel 2',status:'PASS',observations:50},{name:'ANC 60/60',status:'SKIP',reason:'ANC fixture checks disabled'}]};
 const rows=scenarioRows(reportRows(JSON.stringify({status:'PASS',case:'stereo',detail:'rx: '+JSON.stringify(rx)})));
 assert.equal(rows.filter(row=>row.checkName?.includes('ANC')).length,0);
 assert.equal(rows.filter(row=>row.checkName?.endsWith('PCM payload')).length,0);
 assert.equal(rows.filter(row=>row.checkName?.includes('PCM channel')).length,2);
});
