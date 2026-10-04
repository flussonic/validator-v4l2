// SPDX-License-Identifier: MIT
use crate::{validate::Stats, Result};
use std::{fs::OpenOptions, io::Write};
pub fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c < ' ' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
pub fn emit(
    path: Option<&str>,
    status: &str,
    case: &str,
    detail: &str,
    stats: Option<&Stats>,
) -> Result<()> {
    let mut s = format!(
        "{{\"status\":{},\"case\":{},\"detail\":{}",
        quote(status),
        quote(case),
        quote(detail)
    );
    if let Some(v) = stats {
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
