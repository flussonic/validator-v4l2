// SPDX-License-Identifier: MIT
use crate::{
    agent,
    device::*,
    json::{self, Value as V},
    pattern::*,
    report::emit,
    Options, Result, Task,
};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

fn string(value: impl Into<String>) -> V {
    V::Str(value.into())
}

/// Structured capabilities are separate from human-readable inventory reports.
pub fn topology() -> Result<V> {
    let o = Options::from_args(["list".into()])?;
    let mut devices = vec![];
    for path in crate::nodes(&o)? {
        let result = (|| -> Result<V> {
            let info = probe(&path)?;
            let sdi = info.multiplanar();
            Ok(json::object(&[
                ("device", string(&path)),
                ("driver", string(crate::text(&info.driver))),
                ("card", string(crate::text(&info.card))),
                ("bus", string(crate::text(&info.bus))),
                ("sdi", V::Bool(sdi)),
                ("output", V::Bool(info.output != 0)),
                (
                    "modes",
                    V::Array(if sdi {
                        modes(&path)?
                            .into_iter()
                            .map(|m| string(m.name()))
                            .collect()
                    } else {
                        vec![]
                    }),
                ),
                (
                    "formats",
                    V::Array(if sdi {
                        formats(&path, info.output != 0)?
                            .into_iter()
                            .map(|f| string(code(f)))
                            .collect()
                    } else {
                        vec![]
                    }),
                ),
            ]))
        })();
        devices.push(result.unwrap_or_else(|error| {
            json::object(&[("device", string(path)), ("error", string(error))])
        }));
    }
    Ok(json::object(&[
        ("host_id", string(host_id())),
        ("devices", V::Array(devices)),
    ]))
}

fn host_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .hash(&mut hash);
    format!("{:016x}", hash.finish())
}

#[derive(Clone, Debug)]
struct Node {
    url: Option<String>,
    path: String,
    output: bool,
    modes: BTreeSet<String>,
    formats: BTreeSet<String>,
}
impl Node {
    fn label(&self) -> String {
        format!("{} {}", self.url.as_deref().unwrap_or("local"), self.path)
    }
}
fn parse_nodes(value: &V, url: Option<String>) -> Result<Vec<Node>> {
    let mut nodes = vec![];
    for entry in value.get("devices")?.array()? {
        if entry.get("error").is_ok() || !entry.get("sdi")?.boolean()? {
            continue;
        }
        let strings = |key| -> Result<BTreeSet<String>> {
            entry
                .get(key)?
                .array()?
                .iter()
                .map(|v| v.string().map(String::from))
                .collect()
        };
        let path = entry.get("device")?.string()?;
        if !path
            .strip_prefix("/dev/video")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("invalid topology device path".into());
        }
        nodes.push(Node {
            url: url.clone(),
            path: path.into(),
            output: entry.get("output")?.boolean()?,
            modes: strings("modes")?,
            formats: strings("formats")?,
        });
    }
    Ok(nodes)
}
fn endpoint(url: &str, o: &Options) -> Result<agent::Endpoint> {
    agent::Endpoint::new(
        url,
        o.values
            .get("--agent-token")
            .cloned()
            .or_else(|| std::env::var("VALIDATOR_HTTP_TOKEN").ok()),
    )
}
fn routing_options(o: &Options, tx: &Node, rx: &Node) -> Options {
    let mut routed = o.clone();
    for key in [
        "--agent-url",
        "--tx-url",
        "--rx-url",
        "--tx-host",
        "--rx-host",
        "--device",
        "--peer-agent",
    ] {
        routed.values.remove(key);
    }
    if let Some(url) = &tx.url {
        routed.values.insert("--tx-url".into(), url.clone());
    }
    if let Some(url) = &rx.url {
        routed.values.insert("--rx-url".into(), url.clone());
    }
    routed
        .values
        .insert("--pair".into(), format!("{}={}", tx.path, rx.path));
    routed
}
fn probe_case(tx: &Node, rx: &Node) -> Option<(String, String)> {
    let mode = tx
        .modes
        .intersection(&rx.modes)
        .min_by_key(|mode| {
            let rank = if mode.contains("1080p25.") {
                0
            } else if mode.contains("1080i50.") {
                1
            } else if mode.contains("720p50.") {
                2
            } else if mode.contains("576i50.") {
                3
            } else {
                4
            };
            (rank, mode.len(), *mode)
        })?
        .clone();
    let format = ["SDUY", "SD10", "SDYU", "SDYV", "SD16", "SDXU", "SDAR"]
        .into_iter()
        .find(|f| tx.formats.contains(*f) && rx.formats.contains(*f))?
        .to_string();
    Some((mode, format))
}
fn wait(task: &mut Task, other: Option<&Task>, deadline: Instant) -> Result<agent::Output> {
    let mut heartbeat = Instant::now();
    loop {
        if crate::stop_requested() || Instant::now() >= deadline {
            return Err("connection discovery interrupted or timed out".into());
        }
        if heartbeat.elapsed() >= Duration::from_secs(1) {
            task.heartbeat()?;
            if let Some(other) = other {
                other.heartbeat()?;
            }
            heartbeat = Instant::now();
        }
        if let Some(output) = task.poll()? {
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn has_probe(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| json::parse(line).ok())
        .any(|value| {
            value
                .get("probe_frames")
                .and_then(V::integer)
                .is_ok_and(|frames| frames >= 3)
        })
}
fn test_connection(o: &Options, tx: &Node, rx: &Node, id: u32) -> Result<bool> {
    let Some((mode, format)) = probe_case(tx, rx) else {
        return Ok(false);
    };
    let routed = routing_options(o, tx, rx);
    let common = vec![
        "--mode".into(),
        mode,
        "--format".into(),
        format,
        "--probe-id".into(),
        id.to_string(),
        "--no-anc".into(),
        "--no-vbi".into(),
    ];
    // Reopen the transmitter for each candidate: half-duplex inputs can alter its port.
    let mut ta = vec![
        "transmit".into(),
        "--device".into(),
        tx.path.clone(),
        "--frames".into(),
        "1000000".into(),
        "--duration".into(),
        "10".into(),
        "--stop-on-stdin".into(),
    ];
    ta.extend(common.clone());
    let mut transmitter = Task::start(&routed, "--tx-host", &ta)?;
    std::thread::sleep(Duration::from_secs(1));
    if let Some(output) = transmitter.poll()? {
        return Err(format!(
            "discovery transmitter unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut ra = vec![
        "receive".into(),
        "--device".into(),
        rx.path.clone(),
        "--frames".into(),
        "4".into(),
        "--warmup".into(),
        "2".into(),
        "--timeout-ms".into(),
        "800".into(),
    ];
    ra.extend(common);
    let mut receiver = Task::start(&routed, "--rx-host", &ra)?;
    let received = wait(
        &mut receiver,
        Some(&transmitter),
        Instant::now() + Duration::from_secs(5),
    );
    transmitter.stop();
    let transmitted = wait(
        &mut transmitter,
        None,
        Instant::now() + Duration::from_secs(4),
    )?;
    let received = received?;
    let found = has_probe(&received.stdout);
    if found {
        emit(o.report(), "INFO", &format!("connection {} -> {}", tx.label(), rx.label()),
            &format!("Unique video probe {id} received for at least three consecutive frames; routing only, feature validation follows. rx: {} tx: {} {}", String::from_utf8_lossy(&received.stdout), String::from_utf8_lossy(&transmitted.stdout), String::from_utf8_lossy(&transmitted.stderr)), None)?;
    }
    Ok(found)
}

pub fn quickcheck(o: &Options) -> Result<bool> {
    if o.values.contains_key("--pair")
        || o.values.contains_key("--device")
        || o.has("--inventory-only")
    {
        let mut inventory = o.clone();
        inventory
            .switches
            .retain(|s| s != "--inventory-only" && s != "--discover-only");
        inventory.values.remove("--peer-agent");
        return crate::quick(&inventory);
    }
    let mut all = vec![];
    let mut urls = BTreeSet::new();
    for key in ["--agent-url", "--peer-agent"] {
        if let Some(url) = o.values.get(key) {
            urls.insert(url.trim_end_matches('/').to_string());
        }
    }
    let mut ok = true;
    let mut hosts = BTreeSet::new();
    for url in urls {
        let response = endpoint(&url, o)?.request("GET", "/v1/topology", V::Null)?;
        let output = agent::Output::from_value(&response)?;
        if !output.success {
            return Err(format!(
                "agent topology: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let value = json::parse(&String::from_utf8(output.stdout).map_err(|e| e.to_string())?)?;
        if !hosts.insert(value.get("host_id")?.string()?.to_string()) {
            continue;
        }
        let nodes = parse_nodes(&value, Some(url.clone()))?;
        let mut inventory = o.clone();
        inventory.values.insert("--agent-url".into(), url);
        ok &= crate::inventory(&inventory)?;
        all.extend(nodes);
    }
    if cfg!(target_os = "linux")
        && std::path::Path::new("/sys/class/video4linux").exists()
        && !hosts.contains(&host_id())
    {
        let mut local = o.clone();
        for key in ["--agent-url", "--tx-url", "--rx-url", "--peer-agent"] {
            local.values.remove(key);
        }
        ok &= crate::inventory(&local)?;
        all.extend(parse_nodes(&topology()?, None)?);
    }
    if all.is_empty() {
        emit(
            o.report(),
            "SKIP",
            "automatic connection discovery",
            "No usable five-plane V4L2 devices were found",
            None,
        )?;
    }
    let outputs: Vec<_> = all.iter().filter(|n| n.output).collect();
    let inputs: Vec<_> = all.iter().filter(|n| !n.output).collect();
    let mut connections = vec![];
    let mut covered = BTreeSet::new();
    let base = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32)
        .max(1);
    let mut index = 0u32;
    for tx in outputs {
        for rx in &inputs {
            if crate::stop_requested() {
                return Err("connection discovery interrupted".into());
            }
            if probe_case(tx, rx).is_none() {
                continue;
            }
            index = index.wrapping_add(1);
            eprintln!("Discovering {} -> {}", tx.label(), rx.label());
            match test_connection(o, tx, rx, base.wrapping_add(index).max(1)) {
                Ok(true) => {
                    covered.insert(tx.label());
                    covered.insert(rx.label());
                    connections.push((tx, *rx));
                }
                Ok(false) => {}
                Err(error) => {
                    emit(
                        o.report(),
                        "INFO",
                        &format!("discovery {} -> {}", tx.label(), rx.label()),
                        &error,
                        None,
                    )?;
                }
            }
        }
    }
    emit(o.report(), "INFO", "automatic discovery summary",
        &format!("{} nodes, {} compatible output/input candidates probed, {} identified connections. Missing routes have no end-to-end coverage.", all.len(), index, connections.len()), None)?;
    for node in &all {
        if !covered.contains(&node.label()) {
            emit(o.report(), "SKIP", &node.label(), "No connection carrying this run's video probe was found; end-to-end feature coverage unavailable", None)?;
            if !node.output && !o.has("--discover-only") {
                let routed = routing_options(o, node, node);
                let format = node.formats.iter().find(|f| FORMATS.contains(&f.as_str()));
                if let Some(format) = format {
                    let args = vec![
                        "receive".into(),
                        "--device".into(),
                        node.path.clone(),
                        "--format".into(),
                        format.clone(),
                        "--frames".into(),
                        "8".into(),
                        "--timeout-ms".into(),
                        "800".into(),
                    ];
                    let result = crate::endpoint_output(&routed, "--rx-host", &args)?;
                    let stderr = String::from_utf8_lossy(&result.stderr);
                    let unavailable = ["errno 67", "errno 61", "errno 11", "errno 16"]
                        .iter()
                        .any(|e| stderr.contains(e));
                    let status = if result.success {
                        "OBSERVED"
                    } else if unavailable {
                        "SKIP"
                    } else {
                        ok = false;
                        "FAIL"
                    };
                    emit(
                        o.report(),
                        status,
                        &format!("external input {}", node.label()),
                        &format!(
                            "No matched test source: rx: {} {}",
                            String::from_utf8_lossy(&result.stdout),
                            stderr
                        ),
                        None,
                    )?;
                }
            }
        }
    }
    if o.has("--discover-only") {
        return Ok(ok);
    }
    for (tx, rx) in connections {
        let routed = routing_options(o, tx, rx);
        let cases = match crate::planned_cases(&routed, &tx.path) {
            Ok(cases) => cases,
            Err(error) => {
                emit(
                    o.report(),
                    "FAIL",
                    &format!("{} -> {}", tx.label(), rx.label()),
                    &error,
                    None,
                )?;
                ok = false;
                continue;
            }
        };
        for case in cases {
            if crate::stop_requested() {
                return Err("automatic checks interrupted".into());
            }
            if !rx.modes.contains(&case.mode) || !rx.formats.contains(&case.format) {
                emit(
                    o.report(),
                    "SKIP",
                    &format!("{} -> {} {:?}", tx.label(), rx.label(), case),
                    "Receiver does not enumerate this timing/format",
                    None,
                )?;
                continue;
            }
            eprintln!(
                "Checking {} -> {} {} {}",
                tx.label(),
                rx.label(),
                case.mode,
                case.format
            );
            match crate::one_pair(&routed, &case) {
                Ok(pass) => ok &= pass,
                Err(error) => {
                    emit(
                        o.report(),
                        "FAIL",
                        &format!("{} -> {} {:?}", tx.label(), rx.label(), case),
                        &error,
                        None,
                    )?;
                    ok = false;
                }
            }
        }
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_requires_consecutive_identity_evidence_not_success_status() {
        assert!(!has_probe(br#"{"status":"PASS","frames":4}"#));
        assert!(!has_probe(br#"{"probe_frames":2}"#));
        assert!(has_probe(
            br#"{"status":"FAIL","probe_frames":4,"crc_errors":2}"#
        ));
    }
    #[test]
    fn automatic_routes_keep_server_identity_and_use_common_capabilities() {
        let make = |url: &str, output| Node {
            url: Some(url.into()),
            path: "/dev/video0".into(),
            output,
            modes: ["1920x1080p25.000".into(), "3840x2160p50.000".into()].into(),
            formats: ["SDUY".into()].into(),
        };
        let tx = make("http://sender:5040", true);
        let rx = make("http://receiver:5040", false);
        assert_eq!(probe_case(&tx, &rx).unwrap().0, "1920x1080p25.000");
        let o = Options::from_args([
            "quickcheck".into(),
            "--agent".into(),
            "http://sender:5040".into(),
        ])
        .unwrap();
        let routed = routing_options(&o, &tx, &rx);
        assert_eq!(routed.get("--tx-url", ""), "http://sender:5040");
        assert_eq!(routed.get("--rx-url", ""), "http://receiver:5040");
        assert!(!routed.values.contains_key("--agent-url"));
    }
}
