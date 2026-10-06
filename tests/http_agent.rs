// SPDX-License-Identifier: MIT
use std::{
    io::{BufRead, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[path = "../src/json.rs"]
mod json;
type Result<T> = std::result::Result<T, String>;
struct Agent {
    child: Child,
    address: String,
}
impl Agent {
    fn start(token: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"));
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(token) = token {
            command.args(["--token", token]);
        }
        let child = command.spawn().unwrap();
        let mut agent = Self {
            child,
            address: String::new(),
        };
        let stderr = agent.child.stderr.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let line = std::io::BufReader::new(stderr).lines().next();
            let _ = send.send(line);
        });
        let line = receive
            .recv_timeout(Duration::from_secs(5))
            .expect("agent did not report readiness")
            .expect("agent exited before readiness")
            .expect("cannot read agent readiness");
        agent.address = line
            .strip_prefix("validator HTTP agent listening on ")
            .expect("unexpected agent startup message")
            .to_string();

        agent
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &str,
        token: Option<&str>,
    ) -> (u16, json::Value) {
        let mut s = TcpStream::connect(&self.address).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let auth = token.map_or(String::new(), |s| format!("Authorization: Bearer {s}\r\n"));
        write!(s,"{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        let mut response = String::new();
        s.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            json::parse(body).unwrap(),
        )
    }
    fn start_job(&self, body: &str) -> String {
        let (code, v) = self.request("POST", "/v1/jobs", body, None);
        assert_eq!(code, 200, "{v:?}");
        v.get("id").unwrap().string().unwrap().into()
    }
    fn wait(&self, id: &str) -> json::Value {
        let start = Instant::now();
        loop {
            let (_, v) = self.request("GET", &format!("/v1/jobs/{id}"), "", None);
            if !v.get("running").unwrap().boolean().unwrap() {
                return v;
            }
            assert!(start.elapsed() < Duration::from_secs(6), "{v:?}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn real_agent_executes_validator_and_preserves_results_without_ssh() {
    let agent = Agent::start(None);
    let (_, health) = agent.request("GET", "/v1/health", "", None);
    assert_eq!(health.get("api").unwrap().integer().unwrap(), 1);
    let (_, jobs) = agent.request("GET", "/v1/jobs", "", None);
    assert!(jobs.array().unwrap().is_empty());
    let id = agent.start_job(
        // This checks agent execution/results; the software suite covers every format separately.
        r#"{"command":"software","args":["--frames","1","--format","SDUY"],"lease_secs":15,"run_id":"integration"}"#,
    );
    let result = agent.wait(&id);
    assert_eq!(result.get("exit_code").unwrap().integer().unwrap(), 0);
    assert_eq!(
        result.get("run_id").unwrap().string().unwrap(),
        "integration"
    );
    assert!(result
        .get("stdout")
        .unwrap()
        .string()
        .unwrap()
        .contains("\"status\":\"PASS\""));
}
#[test]
fn abandoned_tasks_expire_and_expired_success_is_not_relabelled_completed() {
    let agent = Agent::start(None);
    let id = agent
        .start_job(r#"{"command":"software","args":["--frames","1000000000"],"lease_secs":1}"#);
    let result = agent.wait(&id);
    assert_eq!(
        result.get("reason").unwrap().string().unwrap(),
        "lease_expired"
    );
}
#[test]
fn heartbeats_keep_work_alive_and_stop_releases_it() {
    let agent = Agent::start(None);
    let id = agent
        .start_job(r#"{"command":"software","args":["--frames","1000000000"],"lease_secs":1}"#);
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(300));
        let (code, v) = agent.request(
            "POST",
            &format!("/v1/jobs/{id}/heartbeat"),
            r#"{"lease_secs":1}"#,
            None,
        );
        assert_eq!(code, 200);
        assert!(v.get("running").unwrap().boolean().unwrap());
    }
    let (code, _) = agent.request("POST", &format!("/v1/jobs/{id}/stop"), "null", None);
    assert_eq!(code, 200);
    let result = agent.wait(&id);
    assert_eq!(result.get("reason").unwrap().string().unwrap(), "requested");
}
#[test]
fn agent_rejects_shell_commands_file_writes_and_non_video_devices() {
    let agent = Agent::start(None);
    for body in [
        r#"{"command":"sh","args":[]}"#,
        r#"{"command":"receive","args":["--device","/dev/mem"]}"#,
        r#"{"command":"software","args":["--report","/tmp/forbidden"]}"#,
        r#"{"command":"software","args":[],"lease_secs":0}"#,
        r#"{"command":"software","command":"transmit","args":[]}"#,
    ] {
        let (code, _) = agent.request("POST", "/v1/jobs", body, None);
        assert_eq!(code, 400, "{body}");
    }
}
#[test]
fn agent_authentication_applies_to_discovery_and_commands() {
    let agent = Agent::start(Some("test-token"));
    assert_eq!(agent.request("GET", "/v1/health", "", None).0, 401);
    assert_eq!(agent.request("GET", "/v1/health", "", Some("wrong")).0, 401);
    assert_eq!(
        agent.request("GET", "/v1/health", "", Some("test-token")).0,
        200
    );
}

#[test]
fn coordinator_uses_two_agents_and_writes_failure_report_after_worker_errors() {
    let tx = Agent::start(None);
    let rx = Agent::start(None);
    let dir =
        std::env::temp_dir().join(format!("validator-http-coordinator-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("run.jsonl");
    let html = dir.join("run.html");
    let output = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .args([
            "loop",
            "--tx-url",
            &format!("http://{}", tx.address),
            "--rx-url",
            &format!("http://{}", rx.address),
            "--pair",
            "/dev/video999=/dev/video998",
            "--frames",
            "1",
            "--report",
        ])
        .arg(&log)
        .arg("--html")
        .arg(&html)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("\"status\":\"FAIL\""));
    assert!(html.exists());
    for agent in [&tx, &rx] {
        let (_, jobs) = agent.request("GET", "/v1/jobs", "", None);
        let jobs = jobs.array().unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(!jobs[0].get("running").unwrap().boolean().unwrap());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn quickcheck_agent_runs_remotely_and_saves_report() {
    let agent = Agent::start(None);
    let dir = std::env::temp_dir().join(format!("validator-quickcheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("quick.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .args([
            "quickcheck",
            "--agent",
            &format!("http://{}", agent.address),
            "--device",
            "/dev/video999999",
            "--report",
        ])
        .arg(&log)
        .arg("--html")
        .arg(dir.join("quick.html"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let (_, jobs) = agent.request("GET", "/v1/jobs", "", None);
    let jobs = jobs.array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].get("command").unwrap().string().unwrap(), "quick");
    assert!(!jobs[0].get("running").unwrap().boolean().unwrap());
    let log = std::fs::read_to_string(log).unwrap();
    assert!(log.contains("/dev/video999999"));
    assert!(log.contains("\"status\":\"FAIL\""));
    assert!(dir.join("quick.html").exists());
    let exported = std::fs::read_to_string(dir.join("quick.json")).unwrap();
    let records = json::parse(&exported).unwrap();
    assert!(records
        .array()
        .unwrap()
        .iter()
        .any(|r| r.get("status").unwrap().string().unwrap() == "FAIL"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn network_bind_accepts_optional_authentication() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .args(["serve", "--listen", "0.0.0.0:0"])
        .env_remove("VALIDATOR_HTTP_TOKEN")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    thread::spawn(move || {
        use std::io::BufRead;
        let line = std::io::BufReader::new(stderr)
            .lines()
            .next()
            .unwrap()
            .unwrap();
        let _ = send.send(line);
    });
    let line = receive.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    assert!(line
        .unwrap()
        .starts_with("validator HTTP agent listening on 0.0.0.0:"));
}

#[test]
fn bare_quickcheck_produces_html_and_json_even_when_agent_is_unreachable() {
    let dir =
        std::env::temp_dir().join(format!("validator-default-reports-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let out = Command::new(env!("CARGO_BIN_EXE_validator-v4l2"))
        .current_dir(&dir)
        .args(["quickcheck", "--agent", &format!("http://{address}")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|f| f.unwrap().path())
        .collect();
    assert_eq!(files.len(), 3);
    let file = files
        .iter()
        .find(|p| p.extension().unwrap() == "json")
        .unwrap();
    let records = json::parse(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(
        records.array().unwrap()[0]
            .get("status")
            .unwrap()
            .string()
            .unwrap(),
        "FAIL"
    );
    assert!(files.iter().any(|p| p.extension().unwrap() == "html"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn parallel_agents_keep_their_own_bound_ports_and_authentication() {
    let threads: Vec<_> = (0..8)
        .map(|index| {
            thread::spawn(move || {
                let token = format!("parallel-test-token-{index}");
                let agent = Agent::start(Some(&token));
                assert_eq!(agent.request("GET", "/v1/health", "", None).0, 401);
                assert_eq!(agent.request("GET", "/v1/health", "", Some(&token)).0, 200);
                agent
            })
        })
        .collect();
    let agents: Vec<_> = threads
        .into_iter()
        .map(|task| task.join().unwrap())
        .collect();
    let addresses: std::collections::BTreeSet<_> =
        agents.iter().map(|agent| &agent.address).collect();
    assert_eq!(addresses.len(), agents.len());
}
