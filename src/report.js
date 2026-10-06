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
      const operational = [...detail.matchAll(/(?:validator-v4l2: |agent task ended: )([^\n]+)/g)].map(match=>match[1]);
      const errors = [...(Array.isArray(tx?.errors) ? tx.errors : []), ...(Array.isArray(measured.errors) ? measured.errors : []), ...operational];
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
      {name: match[2], driver: match[1], bus: match[3], device: ref.device, agent: ref.agent, capabilities: row.detail.split(" sysfs=")[0].split(" direction=")[1]});
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
    row.capabilityKey = refs.map(ref => inventory.get((ref.agent || "") + "|" + ref.device)?.capabilities || "").join(" -> ");
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
function fourKCoverage(group) {
  if(group.rows.length && group.rows.every(row=>/mode: "ASI"/.test(row.name)))return '';
  const rows=group.rows.filter(row=>/\b(?:3840|4096)x2160[pi]/.test(row.name));
  if(rows.length) return '4K: ' + new Set(rows.map(row=>row.name)).size + ' сценариев в отчёте, ' + rows.length + ' отдельных результатов. Проверки и причины SKIP приведены ниже.';
  const capabilities=group.rows[0]?.capabilityKey || '';
  const endpoints=capabilities.split(' -> ');
  const unavailable=endpoints.flatMap((caps,index)=> {
    if(!caps || /\b(?:3840|4096)x2160[pi]/.test(caps))return [];
    const heights=[...caps.matchAll(/\d+x(\d+)[pi]/g)].map(match=>Number(match[1]));
    return heights.length ? [(index===0?'TX':'RX') + ' объявляет режимы с высотой не более ' + Math.max(...heights)] : [];
  });
  return '4K: сценарии отсутствуют; приём 4K не проверен.' + (unavailable.length ? ' ' + unavailable.join('; ') + '. 4K не объявлено драйвером.' : ' Наличие поддержки по этому отчёту установить нельзя.');
}
function scenarioRows(rows) {
  return rows.flatMap(row => {
    if (row.status === 'INFO') return [];
    if (row.status === 'SKIP') return [row];
    const tx = parseChild(row.detail, 'tx'), rx = parseChild(row.detail, 'rx');
    const workers = tx || rx ? [['TX', tx], ['RX', rx]] : [['', row.record]];
    const checks = workers.flatMap(([side, worker]) => (worker?.checks || []).map(check => ({side, worker, check})));
    if (!checks.length) return [row]; // Legacy logs remain combined; never invent feature PASSes.
    const relevant = checks.filter(({worker,check}) => {
      if(check.status==='SKIP' && ['ANC fixture checks disabled','PCM scenario','VBI waveform checks disabled','Encoded two-slot carrier; no PCM channels requested'].includes(check.reason)) return false;
      if(check.name==='PCM payload' && worker.checks.some(c=>/^PCM channel /.test(c.name))) {
        if(check.status==='PASS')return false;
        if(check.status==='FAIL' && worker.checks.some(c=>/^PCM channel /.test(c.name) && c.status==='FAIL') && (check.errors || []).every(e=>e.includes('PCM tone/continuity mismatches')))return false;
      }
      return true;
    });
    const result = relevant.map(({side, worker, check}) => ({...row,
      status: ['PASS','FAIL','SKIP'].includes(check.status) ? check.status : 'INVALID',
      checkName: (side ? side + ' · ' : '') + check.name,
      measured: worker, errors: check.errors || [], reason:check.reason || '',
      detail: [check.reason, 'Completed measurements: ' + check.observations, 'Failures: ' + check.failure_count, ...(check.errors || []), row.detail].filter(Boolean).join('\n'),
    }));
    // A worker that never returned structured measurements must not disappear.
    for (const [side, worker] of workers) {
      if (!worker || !worker.checks?.length) result.push({...row, status:worker?.status || (row.status === 'FAIL' ? 'FAIL' : 'SKIP'), measured:worker || row.measured, checkName:side + (worker ? ' · combined worker (legacy)' : ' · stream setup / execution')});
    }
    return result;
  });
}
function routeIdentity(row) {
  const refs = endpointRefs(row.name);
  return refs.map(ref => (ref.agent || 'local') + ' ' + ref.device).join(' -> ');
}
function scenarioLabel(name) {
  const mode = name.match(/mode: "([^"]+)"/), format = name.match(/format: "([^"]+)"/);
  if (!mode) return name;
  if (mode[1] === "ASI") return "ASI · MPEG-TS 188 · memory=" + (name.match(/memory: "([^"]+)"/)?.[1] || "mmap");
  const resolution=mode[1].startsWith('3840x2160') ? '4K UHD · ' : mode[1].startsWith('4096x2160') ? '4K DCI · ' : '';
  const parts = [resolution+mode[1], format?.[1]];
  const channels=Number(name.match(/channels: (\d+)/)?.[1] || 0);
  if(name.includes('eac3: true'))parts.push('E-AC-3 5.1 · SDI 1–2');
  else if(name.includes('nonpcm: true'))parts.push('ST 337M · SDI 1–2');
  else if(channels)parts.push(channels===2 ? 'стерео 1–2' : 'PCM 1–'+channels);
  for(const key of ['flags','alternate','pad']) {
    const value=Number(name.match(new RegExp(key+': ([0-9]+)'))?.[1] || 0);
    if(value)parts.push(key+'='+value);
  }
  const memory=name.match(/memory: "([^"]+)"/)?.[1];
  if(memory && memory!=='mmap')parts.push(memory.toUpperCase());
  if(name.includes('no_meta: true'))parts.push('без выходных метаданных аудио');
  if(name.includes('anc: true'))parts.push(name.includes('scte104_fragments: true') ? 'ANC · SCTE-104 fragments' : 'ANC');
  if(name.includes('vbi: true'))parts.push('VBI');
  return parts.filter(Boolean).join(' · ');
}
function physicalIdentity(row) {
  if (row.boards.length !== 2 || row.boards.some(board => !board.bus || !board.driver)) return null;
  return row.boards.map(board => [board.agent || 'local',board.bus,board.driver].join('|')).join(' -> ') + '|' + row.capabilityKey;
}
function comparisonSignature(group) {
  return JSON.stringify(group.rows.map(row => [scenarioLabel(row.name),row.checkName || 'legacy combined scenario',row.status]).sort((a,b)=>JSON.stringify(a).localeCompare(JSON.stringify(b))));
}
function reportGroups(rawRows) {
  const evidence = new Map(), aliases = new Map(), groups = new Map();
  for (const row of rawRows) {
    if (row.name.startsWith('connection ')) evidence.set(routeIdentity(row), row);
    if (row.name.startsWith('equivalent route ')) {
      try { aliases.set(routeIdentity(row), {...JSON.parse(row.detail),label:row.boardLabel,boards:row.boards}); } catch (_) { /* Keep raw evidence. */ }
    }
  }
  for (const row of scenarioRows(rawRows)) {
    const route = routeIdentity(row) || 'Software / operational checks';
    if (!groups.has(route)) groups.set(route, {route, label:row.boardLabel || route, boards:row.boards, rows:[], connection:evidence.get(route), kind:endpointRefs(row.name).length === 1 && row.status === 'SKIP' ? 'unavailable' : 'validation'});
    groups.get(route).rows.push(row);
  }
  for (const [route, alias] of aliases) {
    if (!groups.has(route)) groups.set(route, {route,label:evidence.get(route)?.boardLabel || alias.label || route, boards:evidence.get(route)?.boards || alias.boards || [], rows:[],connection:evidence.get(route),kind:'cable'});
    groups.get(route).representative = alias.representative;
    groups.get(route).note = alias.tested ? 'Additional cable was tested independently.' : 'Same boards and advertised capabilities. Matrix omitted by default; identical results are not established.';
  }
  const representatives = new Map();
  for (const group of groups.values()) {
    group.counts = reportCounts(group.rows);
    const sample = group.rows[0] || group.connection;
    // Transport family belongs to equivalence: ASI and SDI never collapse together.
    const family = group.label.includes('ASI') || group.rows.some(row=>row.name.includes('mode: "ASI"')) ? 'ASI' : 'SDI';
    const physical = sample && physicalIdentity(sample);
    const identity = physical && physical + '|' + family;
    const base = identity && representatives.get(identity);
    if (group.representative || base) {
      group.kind = 'cable';
      group.representative ||= base?.route;
      const reference = groups.get(group.representative);
      if (reference?.rows.length && group.rows.length) {
        const same = comparisonSignature(reference) === comparisonSignature(group);
        group.comparison = same ? 'Scenario statuses are identical to the representative.' : 'Scenario statuses or coverage differ from the representative.';
      } else group.comparison = 'No matrix on this cable; report identity has not been tested.';
    } else if (identity) representatives.set(identity, group);
    if (group.connection && group.rows.length) group.rows[0] = {...group.rows[0], detail:group.rows[0].detail + '\n\nConnection discovery evidence:\n' + group.connection.detail};
  }
  return {validation:[...groups.values()].filter(group=>group.kind==='validation'), cables:[...groups.values()].filter(group=>group.kind==='cable'), unavailable:[...groups.values()].filter(group=>group.kind==='unavailable')};
}
function scenarioStage(row) {
  const name=row.name;
  if(name.includes('mode: "ASI"'))return {order:9,title:'9. ASI / MPEG-TS'};
  if(name.includes('eac3: true'))return {order:3,title:'3. E-AC-3 5.1 на двухканальном SDI-носителе'};
  if(name.includes('nonpcm: true'))return {order:4,title:'4. ST 337M: транспорт сжатого аудио'};
  const flags=Number(name.match(/flags: (\d+)/)?.[1] || 0);
  if(flags || name.includes('alternate: 25'))return {order:7,title:'7. HDR, цвет и Level A/B'};
  if(name.includes('memory: "userptr"') || name.includes('memory: "dmabuf"') || name.includes('no_meta: true') || /pad: [1-9]/.test(name))return {order:8,title:'8. Буферы и дополнительные параметры'};
  if(name.includes('vbi: true') && name.includes('anc: false'))return {order:6,title:'6. SD VBI / телетекст'};
  if(name.includes('anc: true'))return {order:5,title:'5. ANC: таймкод, SCTE-104, AFD, субтитры'};
  const channels=Number(name.match(/channels: (\d+)/)?.[1] || 0);
  if(channels>2)return {order:2,title:'2. PCM: остальные каналы до 16'};
  if(channels===2)return {order:1,title:'1. Видео + стерео (PCM 1–2)'};
  return {order:10,title:'Другие проверки / старые записи'};
}
function checkLabel(name, row) {
  const prefix=name.match(/^(TX|RX) · /)?.[0] || '';
  const key=name.slice(prefix.length);
  const channel=key.match(/^PCM channel (\d+)$/);
  if(channel)return prefix+'PCM — канал '+channel[1];
  const names={
    'video pattern':'Видео: рисунок и счётчик кадров',
    'PCM payload':'PCM: общая целостность и порядок каналов',
    'audio metadata/cadence':'Аудио: число сэмплов, частота и маски каналов',
    'encoded audio metadata':'Non-PCM: метаданные сжатого аудио на каналах 1–2',
    'encoded audio':row.name.includes('eac3: true')?'E-AC-3: syncframes и payload ST 337/340':'ST 337M: payload сжатого аудио',
    'ANC 60/60':'Таймкод ATC (ANC 60/60)',
    'ANC 41/07':row.name.includes('scte104_fragments: true')?'SCTE-104: фрагментированный пакет (ANC 41/07)':'SCTE-104 (ANC 41/07)',
    'ANC 41/05':'AFD / Bar Data (ANC 41/05)',
    'ANC 43/02':'OP-47 / телетекст (ANC 43/02)',
    'ANC 61/01':'Субтитры CEA-708 (ANC 61/01)',
    'ANC 5f/fa':'Тестовый счётчик кадра (ANC 5f/fa)',
    'ANC structure':'ANC: структура, parity, checksum и расположение',
    'SD VBI':'SD VBI: телетекст / форма сигнала',
    'HDR/level metadata':'HDR, цвет и Level A/B',
    'five-plane ABI':'Структура пяти плоскостей V4L2',
    'stream continuity':'Последовательность кадров, время и ошибки буферов',
    'driver counters':'Счётчики ошибок драйвера',
    'CRC':'Аппаратные сообщения CRC',
  };
  return prefix+(names[key] || key);
}
function checkOrder(row) {
  const key=(row.checkName || '').replace(/^(TX|RX) · /,'');
  if(key==='video pattern')return 0;
  const ch=key.match(/^PCM channel (\d+)$/);if(ch)return 10+Number(ch[1]);
  if(key==='PCM payload')return 30;
  if(key==='audio metadata/cadence')return 31;
  if(key==='encoded audio')return 1;
  if(key==='encoded audio metadata')return 2;
  if(key==='ANC 60/60')return 40;
  if(key==='ANC 41/07')return 41;
  return 50+(row.checkName?.startsWith('TX')?10:0);
}
function failureSummary(row) {
  const message=row.errors?.[0] || row.reason || (row.status==='SKIP' ? row.detail : '') || '';
  const cadence=message.match(/audio cadence (\d+), expected ([\d.]+)/);
  if(cadence)return 'Аудио: получено '+cadence[1]+' сэмплов на кадр, ожидалось '+cadence[2]+'.';
  const channel=message.match(/PCM channel (\d+): (\d+)\/(\d+) tone samples/);
  if(channel)return 'Канал '+channel[1]+': '+channel[2]+' из '+channel[3]+' сэмплов не совпали с эталонным тоном или нарушили непрерывность.';
  if(row.status==='PASS') {
    if(/PCM channel/.test(row.checkName || ''))return 'Тон канала и непрерывность сэмплов проверены.';
    if(/video pattern/.test(row.checkName || ''))return 'Тестовая картинка и счётчик кадров совпали.';
    return 'Проверка выполнена; ошибок этой проверки нет.';
  }
  if(row.status==='SKIP')return message || 'Проверка недоступна или не выполнялась; PASS не установлен.';
  if(/video marker missing/.test(message))return 'Не удалось прочесть счётчик тестовой картинки; кадр не совпал с известным источником.';
  if(/system is absent/.test(message))return 'DMABUF не проверен: на сервере нет /dev/dma_heap/system; поток не запускался.';
  if(/poll\/DQBUF/.test(message))return 'Не удалось получить следующий буфер V4L2: '+message+'. По этой записи нельзя установить результат отдельных проверок содержимого.';
  if(/configure\/allocate/.test(message))return 'Ошибка настройки потока или выделения буферов: '+message;
  if(/no valid SMPTE 337M preamble/i.test(message))return 'Не получена ожидаемая синхронизация ST 337M; проверка сжатого payload не прошла.';
  if(/No non-PCM channel mask|not marked as non-PCM/.test(message))return 'Метаданные не пометили каналы 1–2 как сжатое аудио. Это отдельная проверка от payload E-AC-3.';
  if(/expected packet absent/.test(message))return 'Ожидаемый пакет не получен: '+checkLabel(row.checkName || '',row)+'.';
  if(/missing\/corrupt ANC/.test(message))return 'Ожидаемый ANC-пакет не найден или не совпал с тестовым содержимым. Ошибка относится к '+checkLabel(row.checkName || '',row)+'.';
  if(/packet present but payload/.test(message))return 'ANC-пакет получен, но его содержимое не соответствует этому кадру.';
  return message || 'Не удалось завершить проверку; причина и stderr в деталях.';
}
function phaseGroups(rows) {
  const phases=new Map();
  for(const row of rows) {
    const stage=scenarioStage(row);
    if(!phases.has(stage.order))phases.set(stage.order,{...stage,rows:[]});
    phases.get(stage.order).rows.push(row);
  }
  return [...phases.values()].sort((a,b)=>a.order-b.order).map(phase=>({...phase,rows:phase.rows.sort((a,b)=>{
    const scenario=scenarioLabel(a.name).localeCompare(scenarioLabel(b.name),undefined,{numeric:true});
    return scenario || checkOrder(a)-checkOrder(b);
  })}));
}

function renderReport() {
  const raw = reportRows(document.getElementById('log').textContent);
  const filter = document.getElementById('status'), search = document.getElementById('search');
  const countsText = counts => ['PASS','FAIL','SKIP','OBSERVED','INVALID'].filter(status=>counts[status] || ['PASS','FAIL','SKIP'].includes(status)).map(status=>status + ' ' + counts[status]).join(', ');
  const element = (tag,text) => {const node=document.createElement(tag); if(text!==undefined)node.textContent=String(text); return node;};
  function draw() {
    const grouped = reportGroups(raw), counts = reportCounts(grouped.validation.flatMap(group=>group.rows));
    const summary = document.getElementById('summary'); summary.replaceChildren();
    for (const [status,count] of Object.entries(counts)) {const card=element('div',status + ': ' + count);card.className='count ' + status.toLowerCase();summary.appendChild(card);}
    document.getElementById('coverage').textContent='Individual checks on representative routes. Additional cables have separate counts. Legacy logs retain combined scenarios; feature outcomes cannot be reconstructed safely.';
    for (const [kind,id] of [['validation','route-groups'],['cables','cable-groups'],['unavailable','unavailable-groups']]) {
      const container=document.getElementById(id);container.replaceChildren();
      for(const group of grouped[kind]) {
        const query=search.value.toLowerCase();
        const matches=row=>(!filter.value || row.status===filter.value) && (!query || (group.label+' '+group.route+' '+row.name+' '+scenarioLabel(row.name)+' '+row.checkName+' '+checkLabel(row.checkName || '',row)+' '+row.detail).toLowerCase().includes(query));
        const visible=group.rows.filter(matches);
        if(group.rows.length && !visible.length)continue;
        if(!group.rows.length && (filter.value || query && !(group.label+' '+group.route).toLowerCase().includes(query)))continue;
        const section=element('details');section.className='route-group';
        const title=element('summary',group.label + ': ' + countsText(group.counts));section.appendChild(title);
        section.appendChild(element('p',group.route));
        const resolution=kind==='validation' ? fourKCoverage(group) : '';
        if(resolution)section.appendChild(element('p',resolution));
        if(group.note)section.appendChild(element('p',group.note));
        if(group.representative)section.appendChild(element('p','Representative: ' + group.representative));
        if(group.comparison)section.appendChild(element('p',group.comparison));
        if(!group.rows.length) { section.appendChild(element('p','Discovery only. No feature validation cases were executed.'));container.appendChild(section);continue; }
        for(const phase of phaseGroups(visible)) {
          section.appendChild(element('h3',phase.title));
          const wrap=element('div');wrap.className='table-wrap';const table=element('table'),head=element('thead'),header=element('tr');
          for(const label of ['Режим / сценарий','Проверка','Результат','Что именно произошло','Кадры / пропуски','Подробности'])header.appendChild(element('th',label));
          head.appendChild(header);table.appendChild(head);const body=element('tbody');
          for(const item of phase.rows) {
            const row=element('tr');row.appendChild(element('td',scenarioLabel(item.name)));
            row.appendChild(element('td',item.checkName ? checkLabel(item.checkName,item) : item.status==='SKIP' ? 'Недоступный сценарий' : item.errors?.some(error=>/poll\/DQBUF|configure\/allocate|QUERYCAP/.test(error)) ? 'Настройка / выполнение потока' : 'Объединённый сценарий (старый журнал)'));
            const status=element('td',item.status);status.className=item.status.toLowerCase();row.appendChild(status);
            row.appendChild(element('td',failureSummary(item)));
            row.appendChild(element('td',(item.measured.frames ?? '—')+' / '+(item.measured.gaps ?? '—')));
            const cell=element('td'),details=element('details');details.append(element('summary','Измерения, ошибки и соединение'),element('pre',item.detail));cell.appendChild(details);row.appendChild(cell);body.appendChild(row);
          }
          table.appendChild(body);wrap.appendChild(table);section.appendChild(wrap);
        }
        container.appendChild(section);
      }
    }
    const diagnostics=document.getElementById('diagnostics');diagnostics.replaceChildren();
    for(const row of raw.filter(row=>row.status==='INFO' && !row.name.startsWith('connection ') && !row.name.startsWith('equivalent route '))) {
      const entry=element('details');entry.append(element('summary',row.boardLabel || row.name),element('pre',row.detail));diagnostics.appendChild(entry);
    }
  }
  filter.addEventListener('change',draw);search.addEventListener('input',draw);draw();
}
if (typeof document !== 'undefined') renderReport();
if (typeof module !== 'undefined') module.exports = {reportRows, reportCounts, scenarioRows, reportGroups, scenarioStage, checkLabel, failureSummary, phaseGroups, fourKCoverage};
