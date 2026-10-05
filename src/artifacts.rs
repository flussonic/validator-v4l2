// SPDX-License-Identifier: MIT
//! Inspection of validator dumps and short generated SCTE-35 transport captures.
//! These commands have explicit coverage; they do not replace common ABI checks.
use crate::{
    device::{Layout, Mode},
    pattern::{fixtures, fourcc, get32, marker, packets},
    report,
    validate::Stats,
    Options, Result,
};
use std::{collections::BTreeSet, fs, path::Path};

fn types(value: Option<&String>) -> Result<Option<BTreeSet<(u8, u8)>>> {
    value
        .map(|value| {
            value
                .split(',')
                .map(|item| {
                    let (did, sdid) = item
                        .split_once('/')
                        .ok_or("ANC types must be DID/SDID hex pairs")?;
                    let did = u8::from_str_radix(did, 16).map_err(|_| "invalid ANC DID")?;
                    let sdid = u8::from_str_radix(sdid, 16).map_err(|_| "invalid ANC SDID")?;
                    if did == 0 {
                        return Err("ANC DID cannot be zero".into());
                    }
                    Ok((did, sdid))
                })
                .collect()
        })
        .transpose()
}

pub fn inspect_anc(o: &Options) -> Result<bool> {
    if !["1080p25", "1920x1080p25.000"].contains(&o.get("--mode", "1080p25")) {
        return Err("inspect-anc currently supports 1080p25 dumps only".into());
    }
    let root = Path::new(o.required("--dump")?);
    let format = o.get("--format", "SDUY");
    let stride = match format {
        "SDUY" | "SDYU" | "SDYV" => 3840,
        "SD10" => 1920_u32.div_ceil(48) * 128,
        "SD16" | "SDXU" | "SDAR" => 7680,
        _ => return Err("unsupported dump format".into()),
    };
    let layout = Layout {
        width: 1920,
        height: 1080,
        stride,
        fourcc: fourcc(format)?,
        ..Layout::default()
    };
    let mode = Mode {
        width: 1920,
        height: 1080,
        total_lines: 1125,
        num: 25,
        den: 1,
        ..Mode::default()
    };
    let selected = types(o.values.get("--anc-types"))?;
    let mut files: Vec<_> = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|p| p.to_string_lossy().ends_with("-0.bin"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err("no validator video-plane dumps".into());
    }
    let mut stats = Stats::default();
    let mut unknown_location = 0;
    let mut unverified_checksum = 0;
    let mut presence_only = BTreeSet::new();
    for file in files {
        stats.frames += 1;
        let name = file
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("invalid dump filename")?;
        let stem = name.strip_suffix("-0.bin").ok_or("invalid dump filename")?;
        let load =
            |plane| fs::read(root.join(format!("{stem}-{plane}.bin"))).map_err(|e| e.to_string());
        let video = load(0)?;
        if video.len() != stride as usize * 1080 {
            stats.fail(format!("{name}: video length/layout mismatch"));
            continue;
        }
        let data = load(2)?;
        if data.len() > 262144 {
            stats.fail("oversized ANC plane");
            continue;
        }
        let ps = match packets(&data) {
            Ok(p) => p,
            Err(e) => {
                stats.fail(e);
                continue;
            }
        };
        if ps.iter().any(|p| p.line == 0) {
            unknown_location += 1;
        }
        let meta = load(3)?;
        if meta.len() < 128 || get32(&meta, 0)? != 0x30494453 || get32(&meta, 4)? != 4 {
            stats.fail("invalid common metadata");
            continue;
        }
        let vendor = get32(&meta, 116)? as usize;
        if vendor > meta.len() - 128 {
            stats.fail("truncated vendor metadata");
            continue;
        }
        if get32(&meta, 120)? == u32::from_le_bytes(*b"MWEC") {
            if get32(&meta, 124)? != 1 || vendor != 16 {
                stats.fail("unknown Magewell metadata version");
                continue;
            }
            if get32(&meta, 128)? & 4 != 0 {
                unverified_checksum += 1;
            }
            if get32(&meta, 132)? as usize != ps.len() || get32(&meta, 136)? != 0 {
                stats.fail("ANC packet counter mismatch or FIFO overflow");
            }
        }
        for p in &ps {
            *stats
                .anc
                .entry(format!("{:02x}/{:02x}", p.did, p.sdid))
                .or_default() += 1;
        }
        if !o.has("--expect") {
            continue;
        }
        let frame = match marker(&video, layout) {
            Ok(v) => v,
            Err(e) => {
                stats.fail(e);
                continue;
            }
        };
        let wanted = fixtures(frame, mode);
        for p in &wanted {
            let key = (p.did, p.sdid);
            if selected.as_ref().is_some_and(|s| !s.contains(&key)) {
                continue;
            }
            let found: Vec<_> = ps.iter().filter(|v| (v.did, v.sdid) == key).collect();
            if found.is_empty() || found.iter().any(|v| v.data != p.data) {
                stats.fail(format!(
                    "picture {frame}: missing/corrupt ANC {:02x}/{:02x}",
                    p.did, p.sdid
                ));
            }
        }
        if let Some(selected) = &selected {
            for &(did, sdid) in selected {
                if wanted.iter().any(|p| (p.did, p.sdid) == (did, sdid)) {
                    continue;
                }
                presence_only.insert(format!("{did:02x}/{sdid:02x}"));
                if !ps.iter().any(|p| (p.did, p.sdid) == (did, sdid)) {
                    stats.fail(format!("picture {frame}: missing ANC {did:02x}/{sdid:02x}"));
                }
            }
        }
    }
    let ok = stats.failure_count == 0;
    let detail = format!("ANC payload/picture association only; not common ABI compliance; unknown_location_frames={unknown_location} original_checksum_unverified_frames={unverified_checksum} presence_only={presence_only:?}");
    report::emit(
        o.report(),
        if !ok {
            "FAIL"
        } else if o.has("--expect") {
            "PASS"
        } else {
            "OBSERVED"
        },
        "inspect-anc",
        &detail,
        Some(&stats),
    )?;
    Ok(ok)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffffffff_u32;
    for byte in data {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x80000000 != 0 { 0x04c11db7 } else { 0 };
        }
    }
    crc
}

pub(crate) fn ts_events(raw: &[u8]) -> Result<Vec<u32>> {
    let mut events = Vec::new();
    if raw.len() < 188 {
        return Err("short TS capture".into());
    }
    for p in raw.chunks_exact(188) {
        if p[0] != 0x47 {
            return Err("TS sync error".into());
        }
        if p[1] & 0x40 == 0 || p[3] & 0x10 == 0 {
            continue;
        }
        let mut off = 4 + if p[3] & 0x20 != 0 {
            1 + p[4] as usize
        } else {
            0
        };
        if off >= 188 {
            continue;
        }
        off += 1 + p[off] as usize;
        let Some(section) = p.get(off..) else {
            continue;
        };
        if section.first() != Some(&0xfc) {
            continue;
        }
        if section.len() < 3 {
            return Err("short SCTE-35 header".into());
        }
        let size = 3 + (((section[1] & 15) as usize) << 8 | section[2] as usize);
        let s = section
            .get(..size)
            .ok_or("inspect-ts currently requires SCTE-35 sections fitting one packet")?;
        if s.len() < 22 || crc32(s) != 0 {
            return Err("SCTE-35 length/CRC error".into());
        }
        if s[3] != 0 || s[4] & 0x80 != 0 || s[13] != 5 || s[18] & 0x80 != 0 {
            return Err(
                "inspect-ts fixture check requires unencrypted, uncancelled splice_insert".into(),
            );
        }
        events.push(u32::from_be_bytes(s[14..18].try_into().unwrap()));
    }
    if events.is_empty() {
        return Err("no generated SCTE-35 splice events".into());
    }
    Ok(events)
}

fn capture_http(url: &str, seconds: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};
    if seconds == 0 || url.contains(['\r', '\n']) {
        return Err("HTTP capture requires a bounded duration and a valid URL".into());
    }
    let uri = url
        .strip_prefix("http://")
        .ok_or("inspect-ts URL capture supports plain HTTP")?;
    let (authority, path) = uri.split_once('/').ok_or("HTTP URL requires a path")?;
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:80")
    };
    let mut last = "no resolved HTTP address".to_string();
    let mut connected = None;
    for address in address.to_socket_addrs().map_err(|e| e.to_string())? {
        match TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last = error.to_string(),
        }
    }
    let mut stream = connected.ok_or(last)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let request = format!("GET /{path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut raw = Vec::new();
    while started.elapsed() < Duration::from_secs(seconds) {
        let mut buffer = [0u8; 16384];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buffer[..n]);
                if raw.len() > 256 * 1024 * 1024 {
                    return Err("HTTP capture exceeds 256 MiB; use a shorter duration".into());
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let end = raw
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("incomplete HTTP response headers")?;
    let headers = std::str::from_utf8(&raw[..end]).map_err(|_| "invalid HTTP headers")?;
    if !headers
        .lines()
        .next()
        .is_some_and(|v| v.starts_with("HTTP/1.0 200 ") || v.starts_with("HTTP/1.1 200 "))
    {
        return Err(format!(
            "HTTP capture failed: {}",
            headers.lines().next().unwrap_or("missing status")
        ));
    }
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return Err("unexpected chunked response to HTTP/1.0 capture".into());
    }
    Ok(raw[end + 4..].to_vec())
}

pub fn inspect_ts(o: &Options) -> Result<bool> {
    let raw = match (o.values.get("--file"), o.values.get("--url")) {
        (Some(file), None) => fs::read(file).map_err(|e| e.to_string())?,
        (None, Some(url)) => capture_http(url, o.number("--duration", 5)?)?,
        _ => return Err("inspect-ts requires exactly one of --file or --url".into()),
    };
    let events = match ts_events(&raw) {
        Ok(v) => v,
        Err(e) => {
            report::emit(o.report(), "FAIL", "inspect-ts", &e, None)?;
            return Ok(false);
        }
    };
    let ok = !o.has("--expect") || events.windows(2).all(|p| p[1] == p[0].wrapping_add(1));
    let detail = format!("generated splice_insert section CRCs; sections={} first_event={} last_event={}; continuous_event_ids={ok}; no PMT/timeline certification", events.len(), events[0], events[events.len()-1]);
    report::emit(
        o.report(),
        if !ok {
            "FAIL"
        } else if o.has("--expect") {
            "PASS"
        } else {
            "OBSERVED"
        },
        "inspect-ts",
        &detail,
        None,
    )?;
    Ok(ok)
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
