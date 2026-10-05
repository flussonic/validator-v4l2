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
    let mut s = format!(
        "{{\"run_id\":{},\"status\":{},\"case\":{},\"detail\":{}",
        quote(crate::run_id()),
        quote(status),
        quote(case),
        quote(detail)
    );
    if let Some(v) = stats {
        s.push_str(&format!(",\"crc_errors\":{}", v.crc_errors));
        s.push_str(&format!(",\"frames\":{},\"gaps\":{},\"failure_count\":{},\"audio_samples\":{},\"audio_present_mask\":{},\"audio_present_channels\":{},\"audio_nonzero_channels\":{},\"audio_nonpcm_mask\":{},\"flags\":{},\"vbi_frames\":{},\"hw_timestamp_frames\":{},\"source_events\":{},\"anc\":{{{}}},\"errors\":[{}]",v.frames,v.gaps,v.failure_count,v.audio_samples,v.present,v.present.count_ones(),v.measured.count_ones(),v.nonpcm,v.flags,v.vbi_frames,v.hw_frames,v.source_events,v.anc.iter().map(|(k,v)|format!("{}:{v}",quote(k))).collect::<Vec<_>>().join(","),v.errors.iter().map(|s|quote(s)).collect::<Vec<_>>().join(",")));
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
        crate::json::parse(line).map_err(|e| format!("invalid report line {}: {e}", index + 1))?;
        records.push(format!("  {line}"));
    }
    fs::write(destination, format!("[\n{}\n]\n", records.join(",\n"))).map_err(|e| e.to_string())
}
