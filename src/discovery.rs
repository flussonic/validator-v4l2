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
    collections::{BTreeMap, BTreeSet},
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
                ("asi", V::Bool(info.asi())),
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
        (
            "dma_heap_system",
            V::Bool(std::path::Path::new("/dev/dma_heap/system").exists()),
        ),
        ("devices", V::Array(devices)),
    ]))
}

thread_local! {
    static CAPABILITIES: std::cell::RefCell<BTreeMap<String, V>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}
pub(crate) fn pair_unavailable(o: &Options, case: &crate::Case) -> Result<Option<String>> {
    // SSH workers retain their direct validation path; HTTP/local coordinators
    // know advertised capabilities and can mark unsupported scenarios SKIP.
    if o.values.contains_key("--tx-host") || o.values.contains_key("--rx-host") {
        return Ok(None);
    }
    let (tx, rx) = crate::pair(o)?;
    if case.config.flags & 1 != 0
        && !(case.mode.contains("1080p50")
            || case.mode.contains("1080p59.94")
            || case.mode.contains("1080p60"))
    {
        return Ok(Some(format!(
            "3G Level B is not applicable to timing {}; requires 1080p50/59.94/60",
            case.mode
        )));
    }
    for (key, path, output) in [("--tx-url", tx, true), ("--rx-url", rx, false)] {
        let url = o.get(key, "local");
        let value = CAPABILITIES.with(|cache| -> Result<V> {
            if let Some(value) = cache.borrow().get(url) {
                return Ok(value.clone());
            }
            let value = if url == "local" {
                topology()?
            } else {
                let response = endpoint(url, o)?.request("GET", "/v1/topology", V::Null)?;
                let response = agent::Output::from_value(&response)?;
                if !response.success {
                    return Err("Cannot obtain device capabilities".into());
                }
                json::parse(&String::from_utf8(response.stdout).map_err(|e| e.to_string())?)?
            };
            cache.borrow_mut().insert(url.into(), value.clone());
            Ok(value)
        })?;
        if case.memory == "dmabuf"
            && value.get("dma_heap_system").and_then(V::boolean).ok() == Some(false)
        {
            return Ok(Some(format!("{url}: DMABUF scenario unavailable; /dev/dma_heap/system is absent. No stream started.")));
        }
        let Some(device) = value
            .get("devices")?
            .array()?
            .iter()
            .find(|d| d.get("device").and_then(V::string).ok() == Some(path))
        else {
            continue;
        };
        if device.get("error").is_ok() {
            continue;
        }
        if device.get("output")?.boolean()? != output {
            return Ok(Some(format!(
                "{url} {path}: requested {} direction is not advertised",
                if output { "OUT" } else { "IN" }
            )));
        }
        if case.mode == "ASI" {
            if !device.get("asi").and_then(V::boolean).unwrap_or(false) {
                return Ok(Some(format!(
                    "{url} {path}: ASI transport is not advertised"
                )));
            }
        } else {
            let supported = device
                .get("modes")?
                .array()?
                .iter()
                .any(|m| m.string().is_ok_and(|m| mode_matches(m, &case.mode)));
            if !supported {
                return Ok(Some(format!(
                    "{url} {path}: timing {} is not advertised",
                    case.mode
                )));
            }
            if !device
                .get("formats")?
                .array()?
                .iter()
                .any(|f| f.string().ok() == Some(case.format.as_str()))
            {
                return Ok(Some(format!(
                    "{url} {path}: format {} is not advertised",
                    case.format
                )));
            }
        }
    }
    Ok(None)
}
fn mode_matches(actual: &str, requested: &str) -> bool {
    fn normalize(mode: &str) -> String {
        if let Some((integer, fraction)) = mode.split_once('.') {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                integer.into()
            } else {
                format!("{integer}.{fraction}")
            }
        } else {
            mode.into()
        }
    }
    normalize(actual) == normalize(requested)
        || actual
            .rsplit_once('x')
            .is_some_and(|(_, short)| normalize(short) == normalize(requested))
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
    board: String,
    asi: bool,
}
impl Node {
    fn label(&self) -> String {
        format!("{} {}", self.url.as_deref().unwrap_or("local"), self.path)
    }
}
fn parse_nodes(value: &V, url: Option<String>) -> Result<Vec<Node>> {
    let mut nodes = vec![];
    for entry in value.get("devices")?.array()? {
        if entry.get("error").is_ok()
            || (!entry.get("sdi")?.boolean()?
                && !entry.get("asi").and_then(V::boolean).unwrap_or(false))
        {
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
        let asi = entry.get("asi").and_then(V::boolean).unwrap_or(false);
        nodes.push(Node {
            asi,
            board: format!(
                "{}|{}|{}",
                value.get("host_id")?.string()?,
                entry.get("bus")?.string()?,
                entry.get("driver")?.string()?
            ),
            url: url.clone(),
            path: path.into(),
            output: entry.get("output")?.boolean()?,
            modes: if asi {
                ["ASI".into()].into()
            } else {
                strings("modes")?
            },
            formats: if asi {
                ["MPEG".into()].into()
            } else {
                strings("formats")?
            },
        });
    }
    Ok(nodes)
}
fn missing_roles(nodes: &[Node]) -> Vec<(&Node, &'static str)> {
    let mut boards: BTreeMap<&str, Vec<&Node>> = BTreeMap::new();
    for node in nodes {
        if !node.board.contains("||") {
            boards.entry(&node.board).or_default().push(node);
        }
    }
    let mut missing = vec![];
    for ports in boards.values() {
        for (asi, output, role) in [
            (false, false, "SDI input"),
            (false, true, "SDI output"),
            (true, false, "ASI input"),
            (true, true, "ASI output"),
        ] {
            if !ports
                .iter()
                .any(|node| node.asi == asi && node.output == output)
            {
                missing.push((ports[0], role));
            }
        }
    }
    missing
}
fn report_missing_roles(o: &Options, nodes: &[Node]) -> Result<()> {
    for (node, role) in missing_roles(nodes) {
        emit(o.report(), "SKIP", &format!("{} capability {role}", node.label()),
            &format!("{role} is not advertised by any inventoried port of this physical board; the scenario is unavailable, not a validation failure"), None)?;
    }
    Ok(())
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
    if tx.asi != rx.asi {
        return None;
    }
    if tx.asi {
        return Some(("ASI".into(), "MPEG".into()));
    }
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
    let mut common = vec![
        "--mode".into(),
        mode,
        "--format".into(),
        format,
        "--probe-id".into(),
        id.to_string(),
        "--no-anc".into(),
        "--no-vbi".into(),
    ];
    if tx.asi {
        common = vec!["--asi".into(), "--probe-id".into(), id.to_string()];
    }
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
            .retain(|s| s != "--inventory-only" && s != "--discover-only" && s != "--all-routes");
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
        report_missing_roles(o, &nodes)?;
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
        let nodes = parse_nodes(&topology()?, None)?;
        report_missing_roles(o, &nodes)?;
        all.extend(nodes);
    }
    if o.has("--asi") {
        all.retain(|node| node.asi);
    }
    if let Some(paths) = o.values.get("--nodes") {
        let paths: BTreeSet<_> = paths.split(',').collect();
        if paths
            .iter()
            .any(|path| !all.iter().any(|node| node.path == *path))
        {
            return Err("--nodes contains an unavailable device node".into());
        }
        all.retain(|node| paths.contains(node.path.as_str()));
    }
    if all.is_empty() {
        emit(
            o.report(),
            "SKIP",
            "automatic connection discovery",
            "No usable V4L2 devices were found for the selected SDI/ASI transport",
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
            if o.has("--cross-board") && tx.board == rx.board {
                continue;
            }
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
    let mut representatives: Vec<(&Node, &Node)> = vec![];
    for (tx, rx) in connections {
        if let Some((base_tx, base_rx)) = representatives
            .iter()
            .find(|(a, b)| equivalent(tx, rx, a, b))
        {
            emit(o.report(), "INFO", &format!("equivalent route {} -> {}", tx.label(), rx.label()),
                &json::object(&[
                    ("representative", string(format!("{} -> {}", base_tx.label(), base_rx.label()))),
                    ("tested", V::Bool(o.has("--all-routes"))),
                    ("reason", string("Same physical boards, direction and advertised capabilities; discovery proves connectivity, not identical validation results")),
                ]).encode(), None)?;
            if !o.has("--all-routes") {
                continue;
            }
        } else {
            representatives.push((tx, rx));
        }
        let routed = routing_options(o, tx, rx);
        let plan = if tx.asi {
            Ok(vec![crate::Case {
                mode: "ASI".into(),
                format: "MPEG".into(),
                config: o.config()?,
                memory: o.get("--memory", "mmap").into(),
            }])
        } else {
            crate::planned_cases(&routed, &tx.path)
        };
        let cases = match plan {
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

fn equivalent(tx: &Node, rx: &Node, a: &Node, b: &Node) -> bool {
    // Missing PCI identity must never collapse independent devices.
    !tx.board.contains("||")
        && !rx.board.contains("||")
        && tx.board == a.board
        && rx.board == b.board
        && tx.asi == a.asi
        && rx.asi == b.asi
        && tx.output == a.output
        && rx.output == b.output
        && tx.modes == a.modes
        && rx.modes == b.modes
        && tx.formats == a.formats
        && rx.formats == b.formats
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
            asi: false,
            board: format!("{url}|PCI:1|driver"),
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

#[cfg(test)]
mod equivalence_tests {
    use super::*;
    fn node(board: &str, path: &str, output: bool) -> Node {
        Node {
            url: Some("http://agent:1".into()),
            path: path.into(),
            board: board.into(),
            output,
            asi: false,
            modes: ["1080p25".into()].into(),
            formats: ["SDUY".into()].into(),
        }
    }
    #[test]
    fn capture_only_board_has_explicit_output_and_asi_skips() {
        let port = node("host|PCI:1|x", "/dev/video0", false);
        let nodes = vec![port];
        let roles: Vec<_> = missing_roles(&nodes)
            .into_iter()
            .map(|(_, role)| role)
            .collect();
        assert_eq!(roles, ["SDI output", "ASI input", "ASI output"]);
    }
    #[test]
    fn cables_share_matrix_only_for_same_physical_boards_direction_and_capabilities() {
        let a = node("host|PCI:1|x", "/dev/video0", true);
        let b = node("host|PCI:2|y", "/dev/video2", false);
        let mut c = node("host|PCI:1|x", "/dev/video4", true);
        let d = node("host|PCI:2|y", "/dev/video6", false);
        assert!(equivalent(&a, &b, &c, &d));
        c.board = "host|PCI:3|x".into();
        assert!(!equivalent(&a, &b, &c, &d));
        c.board = a.board.clone();
        c.asi = true;
        assert!(!equivalent(&a, &b, &c, &d));
        c.asi = false;
        c.formats.insert("SD10".into());
        assert!(!equivalent(&a, &b, &c, &d));
        c.formats = a.formats.clone();
        c.output = false;
        assert!(!equivalent(&a, &b, &c, &d));
    }
}
