// SPDX-License-Identifier: MIT
mod device;
mod pattern;
mod report;
mod sapsan;
mod validate;
use device::*;
use pattern::*;
use report::emit;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use validate::Stats;
pub type Result<T> = std::result::Result<T, String>;
const HELP: &str = r#"validator-v4l2 — common SDI V4L2 compliance and endurance validator

  validator-v4l2 list
  validator-v4l2 quick [--pair /dev/video4=/dev/video0] [--report result.jsonl]
  validator-v4l2 transmit --device /dev/video4 --mode 1080p25 [options]
  validator-v4l2 receive --device /dev/video0 [--expect] [options]
  validator-v4l2 loop --pair /dev/video4=/dev/video0 [options]
  validator-v4l2 soak --pair /dev/video4=/dev/video0 --duration 86400 [options]
  validator-v4l2 software [options]
  validator-v4l2 plan --pair /dev/video4=/dev/video0

Options:
  --frames N           frames per case (default 50)
  --mode NAME          enumerated timing: 1080p25 or 3840x2160p50.000
  --format FOURCC      default SDUY; SDYU SDYV SD16 SD10 SDAR SDXU
  --channels N         generated/expected PCM channels, 1..16 (default 16)
  --flags N            SDI metadata flags, decimal or 0xNN
  --alternate N        alternate flags every N frames, within a stream
  --no-anc --no-vbi    exclude ANC or VBI from generated/expected payload
  --nonpcm             SMPTE 337M transport fixture on channels 1 and 2
  --pad N --no-meta    audio padding / missing output metadata tests
  --memory mmap|userptr|dmabuf  buffer allocation (default mmap)
  --slow-ms N          deliberately delay output refill (fault injection)
  --timeout-ms N       per-frame timeout (default 3000)
  --duration SECONDS   bounded transmit/soak duration
  --cycles N          bound soak by matrix cycles as well as duration
  --seed N            reproducible randomized soak order (default 1)
  --tx-host HOST --rx-host HOST  SSH endpoints (omitted = local Linux)
  --remote-bin PATH    remote executable (default validator-v4l2)
  --report PATH        append machine-readable JSON Lines
  --dump DIR           dump all five planes of each received frame
  --device PATH        node for receive/transmit; quick can select one node

quick without a pair inventories every node and captures currently locked inputs.
Only a paired test verifies transmission end to end. software exercises the
userspace generator/validator without testing a board. Exit: 0 success,
1 validation failure, 2 invalid arguments/operational error. SKIP is not PASS.
"#;
#[derive(Clone)]
struct Options {
    cmd: String,
    values: BTreeMap<String, String>,
    switches: Vec<String>,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut it = std::env::args().skip(1);
        let cmd = it.next().unwrap_or("quick".into());
        let mut values = BTreeMap::new();
        let mut switches = vec![];
        while let Some(k) = it.next() {
            if ["--expect", "--no-anc", "--no-vbi", "--nonpcm", "--no-meta"].contains(&k.as_str()) {
                switches.push(k);
            } else if [
                "--device",
                "--pair",
                "--frames",
                "--mode",
                "--format",
                "--channels",
                "--flags",
                "--alternate",
                "--pad",
                "--memory",
                "--slow-ms",
                "--timeout-ms",
                "--duration",
                "--cycles",
                "--seed",
                "--tx-host",
                "--rx-host",
                "--remote-bin",
                "--report",
                "--dump",
            ]
            .contains(&k.as_str())
            {
                let v = it.next().ok_or(format!("missing value for {k}"))?;
                if values.insert(k.clone(), v).is_some() {
                    return Err(format!("duplicate option {k}"));
                }
            } else {
                return Err(format!("unknown option {k}"));
            }
        }
        let o = Self {
            cmd,
            values,
            switches,
        };
        o.config()?;
        if !["mmap", "userptr", "dmabuf"].contains(&o.get("--memory", "mmap")) {
            return Err("memory must be mmap, userptr or dmabuf".into());
        }
        if o.number("--frames", 50)? == 0 || o.number("--timeout-ms", 3000)? == 0 {
            return Err("frames and timeout must be positive".into());
        }
        Ok(o)
    }
    fn get<'a>(&'a self, k: &str, d: &'a str) -> &'a str {
        self.values.get(k).map_or(d, String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.switches.iter().any(|s| s == k)
    }
    fn number(&self, k: &str, d: u64) -> Result<u64> {
        let Some(s) = self.values.get(k) else {
            return Ok(d);
        };
        if let Some(hex) = s.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else {
            s.parse()
        }
        .map_err(|_| format!("invalid integer for {k}"))
    }
    fn u32(&self, k: &str, d: u32) -> Result<u32> {
        self.number(k, d as u64)?
            .try_into()
            .map_err(|_| format!("{k} exceeds u32"))
    }
    fn config(&self) -> Result<Config> {
        let channels = self.u32("--channels", 16)?;
        if !(1..=16).contains(&channels) {
            return Err("channels must be 1..16".into());
        }
        let flags = self.u32("--flags", 0)?;
        if flags & !63 != 0 || flags & 48 == 48 {
            return Err("invalid metadata flags (HLG and PQ are exclusive)".into());
        }
        if self.has("--nonpcm") && channels < 2 {
            return Err("nonpcm needs at least two channels".into());
        }
        let pad = self.u32("--pad", 0)?;
        if pad > 2048 {
            return Err("padding exceeds audio plane capacity".into());
        }
        Ok(Config {
            flags,
            channels,
            alternate: self.u32("--alternate", 0)?,
            pad,
            no_meta: self.has("--no-meta"),
            nonpcm: self.has("--nonpcm"),
            anc: !self.has("--no-anc"),
            vbi: !self.has("--no-vbi"),
        })
    }
    fn report(&self) -> Option<&str> {
        self.values.get("--report").map(String::as_str)
    }
    fn required(&self, k: &str) -> Result<&str> {
        self.values
            .get(k)
            .map(String::as_str)
            .ok_or(format!("{k} is required"))
    }
}
fn nodes(o: &Options) -> Result<Vec<String>> {
    if let Some(d) = o.values.get("--device") {
        return Ok(vec![d.clone()]);
    }
    let mut v = fs::read_dir("/sys/class/video4linux")
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| format!("/dev/{}", e.file_name().to_string_lossy()))
        .collect::<Vec<_>>();
    v.sort_by_key(|p| {
        p.trim_start_matches("/dev/video")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    Ok(v)
}
const ATTRS: [&str; 23] = [
    "frames",
    "frames_skipped",
    "no_buffer",
    "resyncs",
    "no_sync",
    "events_missed",
    "crc_errors",
    "dma_errors",
    "restarts",
    "underflows",
    "anc_dropped",
    "audio_dropped",
    "signal",
    "reference",
    "timing",
    "reference_offset",
    "clock_adjust",
    "level_a",
    "colorimetry",
    "eotf",
    "idle",
    "hdmi",
    "hdmi_sink",
];
fn sysdir(p: &str) -> PathBuf {
    PathBuf::from("/sys/class/video4linux")
        .join(std::path::Path::new(p).file_name().unwrap_or_default())
}
fn attrs(p: &str) -> BTreeMap<String, String> {
    ATTRS
        .iter()
        .filter_map(|k| {
            fs::read_to_string(sysdir(p).join(k))
                .ok()
                .map(|v| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}
fn inventory(o: &Options) -> Result<()> {
    for p in nodes(o)? {
        match probe(&p) {
            Ok(i) => {
                contract(&p, i.output != 0)?;
                let modes = modes(&p)?;
                let f = formats(&p, i.output != 0)?;
                let detail = format!(
                    "driver={} card={} bus={} direction={} formats={} timings={} sysfs={:?}",
                    text(&i.driver),
                    text(&i.card),
                    text(&i.bus),
                    if i.output != 0 { "output" } else { "capture" },
                    f.iter().map(|f| code(*f)).collect::<Vec<_>>().join(","),
                    modes.iter().map(|m| m.name()).collect::<Vec<_>>().join(","),
                    attrs(&p)
                );
                emit(o.report(), "INFO", &p, &detail, None)?;
            }
            Err(e) => emit(o.report(), "FAIL", &p, &e, None)?,
        }
    }
    Ok(())
}
fn selected(p: &str, name: &str) -> Result<Mode> {
    let ms = modes(p)?;
    for m in ms {
        if m.name() == name {
            return Ok(m);
        }
        let short = format!(
            "{}{}{:.2}",
            m.height,
            if m.interlaced != 0 { 'i' } else { 'p' },
            m.fps() * if m.interlaced != 0 { 2. } else { 1. }
        );
        let stripped = short.trim_end_matches('0').trim_end_matches('.');
        if stripped == name {
            return Ok(m);
        }
        let scan = if m.interlaced != 0 { 'i' } else { 'p' };
        if let Some((h, r)) = name.split_once(scan) {
            if h.parse::<u32>().ok() == Some(m.height)
                && r.parse::<f64>().is_ok_and(|r| {
                    (r - m.fps() * if m.interlaced != 0 { 2. } else { 1. }).abs() < 0.015
                })
            {
                return Ok(m);
            }
        }
    }
    Err(format!("timing {name} is not enumerated by {p}"))
}
struct Settings(Vec<(PathBuf, String)>);
impl Settings {
    fn output(p: &str, c: &Config) -> Result<Self> {
        let mut guard = Self(vec![]);
        for (key, value) in [
            ("level_a", if c.flags & 1 == 0 { "1" } else { "0" }),
            (
                "colorimetry",
                if c.flags & 8 == 0 {
                    "rec709"
                } else {
                    "rec2020"
                },
            ),
            (
                "eotf",
                if c.flags & 16 != 0 {
                    "hlg"
                } else if c.flags & 32 != 0 {
                    "pq"
                } else {
                    "sdr"
                },
            ),
        ] {
            let path = sysdir(p).join(key);
            let Ok(old) = fs::read_to_string(&path) else {
                continue;
            };
            if old.trim() == value {
                continue;
            }
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&path)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode()
                & 0o222
                == 0
            {
                return Err(format!("setting {key} is read-only: {}", old.trim()));
            }
            guard.0.push((path.clone(), old));
            fs::write(&path, value).map_err(|e| format!("set {key}: {e}"))?;
            let got = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if got.trim() != value {
                return Err(format!("{key} readback differs: {}", got.trim()));
            }
        }
        Ok(guard)
    }
}
impl Drop for Settings {
    fn drop(&mut self) {
        for (p, v) in self.0.iter().rev() {
            if let Err(e) = fs::write(p, v) {
                eprintln!("restore {}: {e}", p.display());
            }
        }
    }
}
fn stream(o: &Options, output: bool) -> Result<bool> {
    let p = o.required("--device")?;
    let i = probe(p)?;
    if (i.output != 0) != output || i.caps & (if output { 0x2000 } else { 0x1000 }) == 0 {
        return Err("node direction/common mplane capability mismatch".into());
    }
    let c = o.config()?;
    let mut defaults = c.clone();
    if c.alternate != 0 {
        defaults.flags = 0;
    }
    let _settings = if output {
        Some(Settings::output(p, &defaults)?)
    } else {
        None
    };
    let mode = if output || o.values.contains_key("--mode") {
        Some(selected(p, o.get("--mode", "1080p25"))?)
    } else {
        None
    };
    let d = Device::open(
        p,
        output,
        mode,
        fourcc(o.get("--format", "SDUY"))?,
        match o.get("--memory", "mmap") {
            "userptr" => 1,
            "dmabuf" => 2,
            _ => 0,
        },
    )?;
    let mut gen = Generator::new(c.clone());
    let mut stats = Stats::default();
    let frames = o.number("--frames", 50)?;
    let timeout = o.u32("--timeout-ms", 3000)?;
    let duration = o.number("--duration", 0)?;
    let started = Instant::now();
    let mut produced = 0;
    let mut last_output_seq = None;
    for index in 0..d.count {
        let mut f = d.buffer(index)?;
        if output {
            f.len = gen.fill(f.planes_mut(), d.layout, d.mode)?;
            produced += 1;
        } else {
            f.len = [0; 5];
        }
        d.queue(&f)?;
    }
    d.start()?;
    let baseline = attrs(p);
    let expect = if o.has("--expect") { Some(&c) } else { None };
    while stats.frames < frames
        && (duration == 0 || started.elapsed() < Duration::from_secs(duration))
    {
        let mut f = d.next(timeout)?;
        stats.source_events += f.events as u64;
        if output {
            stats.frames += 1;
            if f.flags & 0x40 != 0 {
                stats.fail("output V4L2_BUF_FLAG_ERROR");
            }
            if last_output_seq.is_some_and(|v| f.sequence.wrapping_sub(v) != 1) {
                stats.fail("output repeated frames");
            }
            last_output_seq = Some(f.sequence);
            let slow = o.number("--slow-ms", 0)?;
            if slow != 0 {
                std::thread::sleep(Duration::from_millis(slow));
            }
            if produced < frames + d.count as u64 {
                let mut b = d.buffer(f.index)?;
                b.len = gen.fill(b.planes_mut(), d.layout, d.mode)?;
                d.queue(&b)?;
                produced += 1;
            }
        } else {
            if let Err(e) = stats.frame(
                f.planes(),
                d.layout,
                d.mode,
                f.sequence,
                f.flags,
                f.timestamp,
                expect,
            ) {
                stats.fail(e);
            }
            if let Some(dir) = o.values.get("--dump") {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                for (n, b) in f.planes().iter().enumerate() {
                    fs::write(
                        PathBuf::from(dir).join(format!(
                            "{}-{}-{}.bin",
                            p.rsplit('/').next().unwrap(),
                            f.sequence,
                            n
                        )),
                        b,
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            f.len = [0; 5];
            d.queue(&f)?;
        }
    }
    let after = attrs(p);
    for key in [
        "frames_skipped",
        "crc_errors",
        "dma_errors",
        "restarts",
        "underflows",
        "anc_dropped",
        "audio_dropped",
    ] {
        if let (Some(a), Some(b)) = (baseline.get(key), after.get(key)) {
            if let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>()) {
                if b > a {
                    stats.fail(format!("sysfs {key} increased by {}", b - a));
                }
            }
        }
    }
    stats.finish(if output { None } else { expect });
    let status = if stats.failure_count != 0 {
        "FAIL"
    } else if output || expect.is_none() {
        "OBSERVED"
    } else {
        "PASS"
    };
    let detail = format!(
        "{} {} {} elapsed={:.3}s counters={:?}",
        if output {
            "output queue completed; reception requires a pair"
        } else {
            "capture"
        },
        d.mode.name(),
        code(d.layout.fourcc),
        started.elapsed().as_secs_f64(),
        after
    );
    emit(o.report(), status, p, &detail, Some(&stats))?;
    Ok(stats.failure_count == 0)
}
#[derive(Clone, Debug)]
struct Case {
    mode: String,
    format: String,
    config: Config,
    memory: String,
}
fn matrix(o: &Options, p: &str) -> Result<Vec<Case>> {
    let ms = if o.values.contains_key("--mode") {
        vec![selected(p, o.get("--mode", ""))?]
    } else {
        modes(p)?
            .into_iter()
            .filter(|m| m.height <= 2160 && m.width <= 4096)
            .collect()
    };
    let fs = if o.values.contains_key("--format") {
        vec![fourcc(o.get("--format", ""))?]
    } else {
        formats(p, true)?
    };
    let mut cases = vec![];
    for m in ms {
        for f in &fs {
            let f = code(*f);
            if !FORMATS.contains(&f.as_str()) {
                continue;
            }
            cases.push(Case {
                mode: m.name(),
                format: f,
                config: o.config()?,
                memory: o.get("--memory", "mmap").into(),
            });
        }
    }
    if !o.values.contains_key("--mode") {
        if let Some(base) = cases
            .iter()
            .find(|c| c.mode.contains("1080p50."))
            .or(cases.first())
            .cloned()
        {
            for flags in [1, 8 | 16, 8 | 32, 1 | 8 | 16] {
                let mut c = base.clone();
                c.config.flags = flags;
                c.config.alternate = 25;
                cases.push(c);
            }
            for channels in [2, 8] {
                let mut c = base.clone();
                c.config.channels = channels;
                cases.push(c);
            }
            let mut c = base.clone();
            c.memory = "userptr".into();
            cases.push(c);
            let mut c = base.clone();
            c.memory = "dmabuf".into();
            cases.push(c);
            let mut c = base.clone();
            c.config.pad = 32;
            cases.push(c);
            let mut c = base.clone();
            c.config.no_meta = true;
            cases.push(c);
            let mut c = base;
            c.config.nonpcm = true;
            cases.push(c);
        }
    }
    Ok(cases)
}
fn case_args(case: &Case) -> Vec<String> {
    let c = &case.config;
    let mut a = vec![
        "--mode".into(),
        case.mode.clone(),
        "--format".into(),
        case.format.clone(),
        "--flags".into(),
        c.flags.to_string(),
        "--channels".into(),
        c.channels.to_string(),
        "--alternate".into(),
        c.alternate.to_string(),
        "--memory".into(),
        case.memory.clone(),
        "--pad".into(),
        c.pad.to_string(),
    ];
    for (on, k) in [
        (!c.anc, "--no-anc"),
        (!c.vbi, "--no-vbi"),
        (c.nonpcm, "--nonpcm"),
        (c.no_meta, "--no-meta"),
    ] {
        if on {
            a.push(k.into());
        }
    }
    a
}
fn command(o: &Options, host_key: &str, args: &[String]) -> Result<Command> {
    if let Some(host) = o.values.get(host_key) {
        if host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err("invalid SSH host".into());
        }
        let mut c = Command::new("ssh");
        let bin = o.get("--remote-bin", "validator-v4l2");
        let quoted = std::iter::once(bin)
            .chain(args.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ");
        c.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "--",
            host,
            &quoted,
        ]);
        Ok(c)
    } else {
        let mut c = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        c.args(args);
        Ok(c)
    }
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn pair(o: &Options) -> Result<(&str, &str)> {
    let (tx, rx) = o
        .required("--pair")?
        .split_once('=')
        .ok_or("pair must be OUTPUT=INPUT")?;
    if tx.is_empty() || rx.is_empty() {
        return Err("empty pair endpoint".into());
    }
    Ok((tx, rx))
}
fn one_pair(o: &Options, case: &Case) -> Result<bool> {
    let (tx, rx) = pair(o)?;
    let mut ta = vec![
        "transmit".into(),
        "--device".into(),
        tx.into(),
        "--frames".into(),
        "1000000000".into(),
    ];
    ta.extend(case_args(case));
    let rate = case
        .mode
        .split(['i', 'p'])
        .next_back()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(25.);
    let duration = (o.number("--frames", 50)? as f64 / rate.max(1.) * 2.).ceil() as u64 + 5;
    ta.extend(["--duration".into(), duration.to_string()]);
    let mut child = command(o, "--tx-host", &ta)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_secs(1));
    let mut ra = vec![
        "receive".into(),
        "--device".into(),
        rx.into(),
        "--expect".into(),
        "--frames".into(),
        o.number("--frames", 50)?.to_string(),
        "--timeout-ms".into(),
        o.u32("--timeout-ms", 3000)?.to_string(),
    ];
    ra.extend(case_args(case));
    // Capture enumeration indices are vendor-specific: the mode name is resolved independently.
    let result = command(o, "--rx-host", &ra)?.output();
    if result.is_err() {
        let _ = child.kill();
    }
    let tx_result = child.wait_with_output().map_err(|e| e.to_string())?;
    let rx_result = result.map_err(|e| e.to_string())?;
    let ok = tx_result.status.success() && rx_result.status.success();
    let detail = format!(
        "tx: {} {} rx: {} {}",
        String::from_utf8_lossy(&tx_result.stdout),
        String::from_utf8_lossy(&tx_result.stderr),
        String::from_utf8_lossy(&rx_result.stdout),
        String::from_utf8_lossy(&rx_result.stderr)
    );
    emit(
        o.report(),
        if ok { "PASS" } else { "FAIL" },
        &format!("{} -> {} {:?}", tx, rx, case),
        &detail,
        None,
    )?;
    Ok(ok)
}
fn quick(o: &Options) -> Result<bool> {
    if o.values.contains_key("--pair") {
        return run_matrix(o, false);
    }
    inventory(o)?;
    let mut ok = true;
    for p in nodes(o)? {
        let i = probe(&p)?;
        if i.output != 0 {
            emit(
                o.report(),
                "SKIP",
                &p,
                "end-to-end output verification needs --pair",
                None,
            )?;
            continue;
        }
        let mut capture = o.clone();
        capture.values.insert("--device".into(), p.clone());
        match stream(&capture, false) {
            Ok(pass) => ok &= pass,
            Err(e)
                if e.contains("errno 67")
                    || e.contains("errno 61")
                    || e.contains("errno 11")
                    || e.contains("errno 16") =>
            {
                emit(
                    o.report(),
                    "SKIP",
                    &p,
                    &format!("input unavailable (busy or no signal): {e}"),
                    None,
                )?
            }
            Err(e) => {
                emit(o.report(), "FAIL", &p, &e, None)?;
                ok = false;
            }
        }
    }
    Ok(ok)
}
fn wire_case(c: &Case) -> String {
    let v = &c.config;
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        c.mode,
        c.format,
        v.flags,
        v.channels,
        v.alternate,
        v.pad,
        v.no_meta as u8,
        v.nonpcm as u8,
        v.anc as u8,
        v.vbi as u8,
        c.memory
    )
}
fn parse_plan(text: &str) -> Result<Vec<Case>> {
    let mut cases = vec![];
    for line in text.lines() {
        let p: Vec<_> = line.split('\t').collect();
        if p.len() != 11 {
            return Err("malformed remote plan".into());
        }
        let n = |i: usize| {
            p[i].parse::<u32>()
                .map_err(|_| "invalid remote plan number".to_string())
        };
        let c = Config {
            flags: n(2)?,
            channels: n(3)?,
            alternate: n(4)?,
            pad: n(5)?,
            no_meta: n(6)? != 0,
            nonpcm: n(7)? != 0,
            anc: n(8)? != 0,
            vbi: n(9)? != 0,
        };
        if !(1..=16).contains(&c.channels)
            || c.pad > 2048
            || c.flags & !63 != 0
            || c.flags & 48 == 48
            || !["mmap", "userptr", "dmabuf"].contains(&p[10])
            || !FORMATS.contains(&p[1])
        {
            return Err("invalid remote plan case".into());
        }
        cases.push(Case {
            mode: p[0].into(),
            format: p[1].into(),
            config: c,
            memory: p[10].into(),
        });
        if cases.len() > 100000 {
            return Err("oversize remote plan".into());
        }
    }
    Ok(cases)
}
fn run_matrix(o: &Options, soak: bool) -> Result<bool> {
    let (tx, _) = pair(o)?;
    let mut cases = if o.values.contains_key("--tx-host") {
        let mut args = vec![
            "plan".into(),
            "--wire-plan".into(),
            "--pair".into(),
            o.required("--pair")?.into(),
        ];
        for key in [
            "--mode",
            "--format",
            "--channels",
            "--flags",
            "--alternate",
            "--pad",
            "--memory",
        ] {
            if let Some(v) = o.values.get(key) {
                args.extend([key.into(), v.clone()]);
            }
        }
        for key in ["--no-anc", "--no-vbi", "--nonpcm", "--no-meta"] {
            if o.has(key) {
                args.push(key.into());
            }
        }
        let result = command(o, "--tx-host", &args)?
            .output()
            .map_err(|e| e.to_string())?;
        if !result.status.success() {
            return Err(format!(
                "remote plan: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        parse_plan(&String::from_utf8(result.stdout).map_err(|e| e.to_string())?)?
    } else {
        matrix(o, tx)?
    };
    if cases.is_empty() {
        return Err("no supported generator modes/formats".into());
    }
    let start = Instant::now();
    let duration = o.number("--duration", if soak { 3600 } else { 0 })?;
    let cycles = o.number("--cycles", if soak { u64::MAX } else { 1 })?;
    let mut seed = o.number("--seed", 1)?;
    let mut ok = true;
    for cycle in 0..cycles {
        if soak {
            for i in (1..cases.len()).rev() {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                cases.swap(i, (seed % (i + 1) as u64) as usize);
            }
        }
        for case in &cases {
            if duration != 0 && start.elapsed().as_secs() >= duration {
                return Ok(ok);
            }
            match one_pair(o, case) {
                Ok(pass) => ok &= pass,
                Err(e) => {
                    emit(
                        o.report(),
                        "FAIL",
                        &format!("cycle {cycle} {case:?}"),
                        &e,
                        None,
                    )?;
                    ok = false;
                }
            }
        }
        if !soak {
            break;
        }
    }
    Ok(ok)
}
fn software(o: &Options) -> Result<bool> {
    let mut ok = true;
    let frames = o.number("--frames", 12)?;
    let fs = if let Some(f) = o.values.get("--format") {
        vec![f.as_str()]
    } else {
        FORMATS.to_vec()
    };
    for (w, h, num, den, lines, interlace) in [
        (720, 576, 25, 1, 625, 1),
        (1920, 1080, 30000, 1001, 1125, 0),
        (3840, 2160, 50, 1, 2250, 0),
    ] {
        let m = Mode {
            width: w,
            height: h,
            interlaced: interlace,
            total_lines: lines,
            num,
            den,
            index: 0,
            reduced: 0,
        };
        for format in &fs {
            let stride = if *format == "SD10" {
                w.div_ceil(48) * 128
            } else {
                w * if ["SD16", "SDXU", "SDAR"].contains(format) {
                    4
                } else {
                    2
                }
            };
            let l = Layout {
                width: w,
                height: h,
                fourcc: fourcc(format)?,
                stride,
                sizes: [stride * h, 262144, 262144, 128, 48960],
            };
            let mut buffers: [Vec<u8>; 5] = std::array::from_fn(|p| vec![0; l.sizes[p] as usize]);
            let mut c = o.config()?;
            c.nonpcm = false;
            c.flags = 24;
            c.alternate = 3;
            let mut gen = Generator::new(c.clone());
            let mut stats = Stats::default();
            for seq in 0..frames {
                let lens = gen.fill(
                    {
                        let [a, b, c, d, e] = &mut buffers;
                        [
                            a.as_mut_slice(),
                            b.as_mut_slice(),
                            c.as_mut_slice(),
                            d.as_mut_slice(),
                            e.as_mut_slice(),
                        ]
                    },
                    l,
                    m,
                )?;
                let p = std::array::from_fn(|i| &buffers[i][..lens[i] as usize]);
                if let Err(e) = stats.frame(
                    p,
                    l,
                    m,
                    seq as u32,
                    0,
                    (seq + 1) * 1000000000 * den / num,
                    Some(&c),
                ) {
                    stats.fail(e);
                }
            }
            stats.finish(Some(&c));
            ok &= stats.failure_count == 0;
            emit(
                o.report(),
                if stats.failure_count == 0 {
                    "PASS"
                } else {
                    "FAIL"
                },
                &format!("software {} {format}", m.name()),
                "userspace round trip; no driver or hardware involved",
                Some(&stats),
            )?;
        }
    }
    Ok(ok)
}
fn run(o: &Options) -> Result<bool> {
    match o.cmd.as_str() {
        "--help" | "help" | "-h" => {
            print!("{HELP}");
            Ok(true)
        }
        "list" => {
            inventory(o)?;
            Ok(true)
        }
        "quick" => quick(o),
        "receive" => stream(o, false),
        "transmit" => stream(o, true),
        "loop" => {
            let c = Case {
                mode: o.get("--mode", "1080p25").into(),
                format: o.get("--format", "SDUY").into(),
                config: o.config()?,
                memory: o.get("--memory", "mmap").into(),
            };
            one_pair(o, &c)
        }
        "soak" => run_matrix(o, true),
        "software" => software(o),
        "plan" => {
            let (tx, _) = pair(o)?;
            for c in matrix(o, tx)? {
                if o.has("--wire-plan") {
                    println!("{}", wire_case(&c));
                } else {
                    emit(
                        o.report(),
                        "PLAN",
                        &format!("{c:?}"),
                        "physical loop or two-server SDI connection required",
                        None,
                    )?;
                }
            }
            Ok(true)
        }
        _ => Err(format!("unknown command {}", o.cmd)),
    }
}
fn main() {
    let result = Options::parse().and_then(|o| run(&o));
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("validator-v4l2: {e}");
            std::process::exit(2)
        }
    }
}
#[cfg(test)]
mod tests;
