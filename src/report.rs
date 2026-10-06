// SPDX-License-Identifier: MIT
pub use crate::json::quote;
use crate::{validate::Stats, Result};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
pub fn emit(
    path: Option<&str>,
    status: &str,
    case: &str,
    detail: &str,
    stats: Option<&Stats>,
) -> Result<()> {
    emit_with_boards(path, status, case, detail, stats, &[])
}
pub fn emit_with_boards(
    path: Option<&str>,
    status: &str,
    case: &str,
    detail: &str,
    stats: Option<&Stats>,
    boards: &[crate::json::Value],
) -> Result<()> {
    let mut s = format!(
        "{{\"run_id\":{},\"status\":{},\"case\":{},\"detail\":{}",
        quote(crate::run_id()),
        quote(status),
        quote(case),
        quote(detail)
    );
    if let Some(v) = stats {
        let checks = v.checks.iter().map(|(name, result)| {
            let status = if result.failures != 0 { "FAIL" } else if result.observations != 0 { "PASS" } else { "SKIP" };
            format!("{{\"name\":{},\"status\":{},\"observations\":{},\"failure_count\":{},\"errors\":[{}],\"reason\":{}}}",
                quote(name), quote(status), result.observations, result.failures,
                result.errors.iter().map(|e| quote(e)).collect::<Vec<_>>().join(","),
                quote(if status == "SKIP" { &result.reason } else { "" }))
        }).collect::<Vec<_>>().join(",");
        s.push_str(&format!(",\"checks\":[{checks}]"));
        s.push_str(&format!(
            ",\"crc_errors\":{},\"probe_frames\":{}",
            v.crc_errors, v.probe_frames
        ));
        s.push_str(&format!(",\"frames\":{},\"gaps\":{},\"failure_count\":{},\"audio_samples\":{},\"audio_present_mask\":{},\"audio_present_channels\":{},\"audio_nonzero_channels\":{},\"audio_nonpcm_mask\":{},\"flags\":{},\"vbi_frames\":{},\"hw_timestamp_frames\":{},\"source_events\":{},\"anc\":{{{}}},\"errors\":[{}]",v.frames,v.gaps,v.failure_count,v.audio_samples,v.present,v.present.count_ones(),v.measured.count_ones(),v.nonpcm,v.flags,v.vbi_frames,v.hw_frames,v.source_events,v.anc.iter().map(|(k,v)|format!("{}:{v}",quote(k))).collect::<Vec<_>>().join(","),v.errors.iter().map(|s|quote(s)).collect::<Vec<_>>().join(",")));
    }
    if stats.is_none() && status != "INFO" && detail.starts_with("tx: ") {
        let checks = pair_checks(detail);
        if !checks.is_empty() {
            s.push_str(&format!(
                ",\"checks\":{}",
                crate::json::Value::Array(checks).encode()
            ));
        }
    }
    if !boards.is_empty() {
        s.push_str(&format!(
            ",\"boards\":{}",
            crate::json::Value::Array(boards.to_vec()).encode()
        ));
    }
    s.push('}');
    println!("{s}");
    if let Some(p) = path {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .map_err(|e| e.to_string())?;
        writeln!(f, "{s}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pair_checks(detail: &str) -> Vec<crate::json::Value> {
    use crate::json::{self, Value as V};
    let mut checks = vec![];
    let mut side = "TX";
    for line in detail.lines() {
        let line = line.trim();
        let content = if let Some(rest) = line.strip_prefix("tx: ") {
            side = "TX";
            rest
        } else if let Some(rest) = line.strip_prefix("rx: ") {
            side = "RX";
            rest
        } else {
            line
        };
        let Ok(record) = json::parse(content) else {
            continue;
        };
        if let Ok(items) = record.get("checks").and_then(V::array) {
            for item in items {
                let mut item = item.clone();
                if let V::Object(fields) = &mut item {
                    fields.insert("side".into(), V::Str(side.into()));
                }
                checks.push(item);
            }
        }
    }
    checks
}

pub fn device_board(info: &crate::device::Info, device: &str) -> crate::json::Value {
    use crate::json::{object, Value as V};
    object(&[
        ("name", V::Str(crate::text(&info.card))),
        ("driver", V::Str(crate::text(&info.driver))),
        ("bus", V::Str(crate::text(&info.bus))),
        ("device", V::Str(device.into())),
        ("agent", V::Null),
    ])
}
pub fn worker_skipped(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        crate::json::parse(line)
            .ok()
            .and_then(|record| {
                record
                    .get("status")
                    .and_then(crate::json::Value::string)
                    .ok()
                    .map(|s| s == "SKIP")
            })
            .unwrap_or(false)
    })
}
pub fn worker_boards(output: &[u8], agent: &str) -> Vec<crate::json::Value> {
    use crate::json::{self, Value as V};
    for line in String::from_utf8_lossy(output).lines() {
        let Ok(record) = json::parse(line) else {
            continue;
        };
        if let Ok(boards) = record.get("boards").and_then(V::array) {
            return boards
                .iter()
                .cloned()
                .map(|mut board| {
                    if let V::Object(fields) = &mut board {
                        fields.insert("agent".into(), V::Str(agent.into()));
                    }
                    board
                })
                .collect();
        }
    }
    vec![]
}

/// Render the saved log without requiring network access or additional packages.
pub fn write_html(source: &str, destination: &str) -> Result<()> {
    check_destination(source, destination)?;
    let log = fs::read_to_string(source).map_err(|e| e.to_string())?;
    let escaped = log
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let page = include_str!("report.html")
        .replacen("{{SCRIPT}}", include_str!("report.js"), 1)
        .replacen("{{LOG}}", &escaped, 1);
    fs::write(destination, page).map_err(|e| e.to_string())
}

fn check_destination(source: &str, destination: &str) -> Result<()> {
    let source_path = fs::canonicalize(source).map_err(|e| e.to_string())?;
    if source_path
        == fs::canonicalize(destination).unwrap_or_else(|_| Path::new(destination).into())
    {
        return Err("Report destination must differ from the JSONL log".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(source), Ok(destination)) =
            (fs::metadata(&source_path), fs::metadata(destination))
        {
            if source.dev() == destination.dev() && source.ino() == destination.ino() {
                return Err("Report destination must differ from the JSONL log".into());
            }
        }
    }
    Ok(())
}

pub fn write_json(source: &str, destination: &str) -> Result<()> {
    check_destination(source, destination)?;
    let log = fs::read_to_string(source).map_err(|e| e.to_string())?;
    let mut records = vec![];
    for (index, line) in log.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        records.push(
            crate::json::parse(line)
                .map_err(|e| format!("invalid report line {}: {e}", index + 1))?,
        );
    }
    add_boards(&mut records);
    let records: Vec<_> = records
        .iter()
        .map(|record| format!("  {}", record.encode()))
        .collect();
    fs::write(destination, format!("[\n{}\n]\n", records.join(",\n"))).map_err(|e| e.to_string())
}

fn endpoints(case: &str) -> Vec<(Option<String>, String)> {
    let words: Vec<_> = case
        .split(|c: char| c.is_whitespace() || c == '=')
        .collect();
    words
        .iter()
        .enumerate()
        .filter_map(|(index, word)| {
            let digits = word.strip_prefix("/dev/video")?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let agent = index.checked_sub(1).and_then(|i| {
                let previous = words[i];
                (previous.starts_with("http://") || previous == "local")
                    .then(|| previous.to_string())
            });
            Some((agent, word.to_string()))
        })
        .collect()
}

/// Preserve server identity: identical device paths on different hosts are different boards.
fn add_boards(records: &mut [crate::json::Value]) {
    use crate::json::{self, Value as V};
    let mut inventory = vec![];
    for record in records.iter() {
        let board = (|| -> Option<_> {
            let detail = record.get("detail").ok()?.string().ok()?;
            let (driver, rest) = detail.strip_prefix("driver=")?.split_once(" card=")?;
            let (name, rest) = rest.split_once(" bus=")?;
            let (bus, _) = rest.split_once(" direction=")?;
            let refs = endpoints(record.get("case").ok()?.string().ok()?);
            if refs.len() != 1 {
                return None;
            }
            let (agent, device) = &refs[0];
            let value = json::object(&[
                ("name", V::Str(name.into())),
                ("driver", V::Str(driver.into())),
                ("bus", V::Str(bus.into())),
                ("device", V::Str(device.clone())),
                (
                    "agent",
                    agent.as_ref().map_or(V::Null, |s| V::Str(s.clone())),
                ),
            ]);
            Some((refs[0].clone(), value))
        })();
        if let Some(board) = board {
            if !inventory.contains(&board) {
                inventory.push(board);
            }
        }
    }
    for record in records {
        let refs = record
            .get("case")
            .and_then(V::string)
            .map(endpoints)
            .unwrap_or_default();
        let explicit = record
            .get("boards")
            .and_then(V::array)
            .unwrap_or(&[])
            .to_vec();
        let mut boards = if refs.is_empty() {
            explicit.clone()
        } else {
            vec![]
        };
        for endpoint in refs {
            let captured: Vec<_> = explicit
                .iter()
                .filter(|board| {
                    let device = board.get("device").and_then(V::string).ok();
                    let agent = board.get("agent").and_then(V::string).ok();
                    device == Some(endpoint.1.as_str())
                        && (agent.is_none()
                            || endpoint.0.is_none()
                            || agent == endpoint.0.as_deref())
                })
                .collect();
            if captured.len() == 1 {
                boards.push(captured[0].clone());
                continue;
            }

            let exact: Vec<_> = inventory
                .iter()
                .filter(|(key, _)| key == &endpoint)
                .collect();
            let candidates: Vec<_> = inventory
                .iter()
                .filter(|(key, _)| key.1 == endpoint.1)
                .collect();
            let board = if exact.len() == 1 {
                Some(&exact[0].1)
            } else if exact.is_empty()
                && candidates.len() == 1
                && (candidates[0].0 .0.is_none() || endpoint.0.is_none())
            {
                Some(&candidates[0].1)
            } else {
                None
            };
            if let Some(board) = board {
                boards.push(board.clone());
            }
        }
        if !boards.is_empty() {
            if let V::Object(fields) = record {
                fields.insert("boards".into(), V::Array(boards));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_routes_keep_board_names_separate_on_identical_device_paths() {
        use crate::json::{object, Value as V};
        let entry = |case: &str, detail: &str| {
            object(&[
                ("case", V::Str(case.into())),
                ("detail", V::Str(detail.into())),
            ])
        };
        let mut records = vec![
            entry(
                "http://sender:5040 /dev/video0",
                "driver=aja card=KONA 5 SDI out 1 bus=synthetic-bus direction=output",
            ),
            entry(
                "http://receiver:5040 /dev/video0",
                "driver=decklink card=DeckLink 8K Pro SDI in 1 bus=synthetic-bus direction=capture",
            ),
            entry(
                "connection http://sender:5040 /dev/video0 -> http://receiver:5040 /dev/video0",
                "video probe",
            ),
            entry("http://unknown:5040 /dev/video0", "unknown host"),
        ];
        add_boards(&mut records);
        let boards = records[2].get("boards").unwrap().array().unwrap();
        assert_eq!(
            boards[0].get("name").unwrap().string().unwrap(),
            "KONA 5 SDI out 1"
        );
        assert_eq!(
            boards[1].get("name").unwrap().string().unwrap(),
            "DeckLink 8K Pro SDI in 1"
        );
        assert!(records[3].get("boards").is_err());
    }
}
