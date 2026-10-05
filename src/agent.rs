// SPDX-License-Identifier: MIT
//! HTTP control of bounded validator tasks; agents never execute a shell.
use crate::{
    json::{self, Value as V},
    Options, Result,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const BODY_LIMIT: usize = 65536;
const OUTPUT_LIMIT: usize = 512 * 1024;
fn text(s: impl Into<String>) -> V {
    V::Str(s.into())
}
fn id() -> String {
    format!(
        "{:x}-{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Clone)]
pub struct Output {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl Output {
    pub(crate) fn from_value(v: &V) -> Result<Self> {
        let code = v.get("exit_code")?;
        let reason = v.get("reason")?.string()?;
        Ok(Self {
            success: *code == V::Int(0) && ["completed", "requested"].contains(&reason),
            stdout: v.get("stdout")?.string()?.as_bytes().to_vec(),
            stderr: format!(
                "{}{}",
                v.get("stderr")?.string()?,
                if ["completed", "requested"].contains(&reason) {
                    String::new()
                } else {
                    format!("\nagent task ended: {reason}")
                }
            )
            .into_bytes(),
        })
    }
}
struct Job {
    id: String,
    run_id: String,
    command: String,
    device: Option<String>,
    lease: Mutex<Instant>,
    stop: AtomicBool,
    overflow: Arc<AtomicBool>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    done: Mutex<Option<(Option<i32>, String)>>,
}
impl Job {
    fn value(&self) -> V {
        let done = lock(&self.done).clone();
        json::object(&[
            ("id", text(&self.id)),
            ("run_id", text(&self.run_id)),
            ("command", text(&self.command)),
            ("device", self.device.as_ref().map_or(V::Null, text)),
            ("running", V::Bool(done.is_none())),
            (
                "exit_code",
                done.as_ref()
                    .and_then(|x| x.0)
                    .map_or(V::Null, |n| V::Int(i64::from(n))),
            ),
            (
                "reason",
                text(done.as_ref().map_or("running", |x| x.1.as_str())),
            ),
            (
                "stdout",
                text(String::from_utf8_lossy(&lock(&self.stdout)).into_owned()),
            ),
            (
                "stderr",
                text(String::from_utf8_lossy(&lock(&self.stderr)).into_owned()),
            ),
        ])
    }
}
struct Server {
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
    token: Option<String>,
    serial: AtomicUsize,
    instance: String,
    binary: std::path::PathBuf,
}
fn capture<R: Read + Send + 'static>(
    mut reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
    overflow: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut block = [0; 8192];
        while let Ok(n) = reader.read(&mut block) {
            if n == 0 {
                break;
            }
            let mut data = lock(&buffer);
            let room = OUTPUT_LIMIT.saturating_sub(data.len());
            data.extend_from_slice(&block[..n.min(room)]);
            if n > room {
                overflow.store(true, Ordering::Relaxed);
            }
        }
    })
}
pub(crate) fn terminate(child: &mut Child, command: &str) {
    if command == "transmit" {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"stop\n");
        }
    } else {
        #[cfg(unix)]
        {
            extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(child.id() as i32, 15);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }
    }
}
fn monitor(mut child: Child, job: Arc<Job>) {
    let out = capture(
        child.stdout.take().expect("piped stdout"),
        job.stdout.clone(),
        job.overflow.clone(),
    );
    let err = capture(
        child.stderr.take().expect("piped stderr"),
        job.stderr.clone(),
        job.overflow.clone(),
    );
    let mut stopping: Option<(Instant, String)> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = out.join();
                let _ = err.join();
                *lock(&job.done) = Some((
                    status.code(),
                    stopping.map_or_else(|| "completed".into(), |x| x.1),
                ));
                break;
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                *lock(&job.done) = Some((None, format!("wait error: {e}")));
                break;
            }
            Ok(None) => {}
        }
        if stopping.is_none() {
            let reason = if job.overflow.load(Ordering::Relaxed) {
                Some("output_limit")
            } else if Instant::now() >= *lock(&job.lease) {
                Some("lease_expired")
            } else if job.stop.load(Ordering::Relaxed) {
                Some("requested")
            } else {
                None
            };
            if let Some(reason) = reason {
                terminate(&mut child, &job.command);
                stopping = Some((Instant::now(), reason.into()));
            }
        }
        if stopping
            .as_ref()
            .is_some_and(|x| x.0.elapsed() > Duration::from_secs(2))
        {
            let _ = child.kill();
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn device(s: &str) -> bool {
    s.strip_prefix("/dev/video")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
fn checked_args(command: &str, args: &[String]) -> Result<Option<String>> {
    if !["transmit", "receive", "software", "quick", "plan"].contains(&command) {
        return Err("unsupported agent command".into());
    }
    if args.len() > 64 || args.iter().any(|s| s.len() > 4096) {
        return Err("argument limit".into());
    }
    let o = Options::from_args(std::iter::once(command.to_string()).chain(args.iter().cloned()))?;
    for k in o.values.keys() {
        if ![
            "--device",
            "--pair",
            "--frames",
            "--warmup",
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
            "--probe-id",
        ]
        .contains(&k.as_str())
        {
            return Err(format!("agent option forbidden: {k}"));
        }
    }
    for flag in &o.switches {
        if ![
            "--expect",
            "--no-anc",
            "--no-vbi",
            "--nonpcm",
            "--eac3",
            "--scte104-fragments",
            "--windowed-audio",
            "--no-meta",
            "--wire-plan",
            "--stop-on-stdin",
            "--exhaustive",
        ]
        .contains(&flag.as_str())
        {
            return Err(format!("agent flag forbidden: {flag}"));
        }
    }
    if command == "quick" {
        if o.values.contains_key("--pair") {
            return Err("paired checks must be coordinated by the client".into());
        }
        if let Some(path) = o.values.get("--device") {
            if !device(path) {
                return Err("agent device must be /dev/videoN".into());
            }
        }
    }
    if command == "plan" {
        let (tx, rx) = crate::pair(&o)?;
        if !device(tx) || !device(rx) {
            return Err("plan requires video device paths".into());
        }
    }
    if ["transmit", "receive"].contains(&command) {
        let path = o.required("--device")?;
        if !device(path) {
            return Err("agent device must be /dev/videoN".into());
        }
        return Ok(Some(path.into()));
    }
    Ok(None)
}
fn lease(v: &V) -> Result<Duration> {
    let n = v.get("lease_secs").map_or(Ok(15), V::integer)?;
    if !(1..=120).contains(&n) {
        return Err("lease_secs must be 1..120".into());
    }
    Ok(Duration::from_secs(n as u64))
}
fn args(v: &V) -> Result<Vec<String>> {
    v.get("args")?
        .array()?
        .iter()
        .map(|s| s.string().map(String::from))
        .collect()
}
impl Server {
    fn start(&self, v: &V) -> Result<V> {
        let command = v.get("command")?.string()?;
        if command == "plan" {
            return Err("use /v1/plan".into());
        }
        let args = args(v)?;
        let device = checked_args(command, &args)?;
        let ttl = lease(v)?;
        let run = v.get("run_id").map_or(Ok(""), V::string)?;
        if run.len() > 128 {
            return Err("run_id limit".into());
        }
        let mut jobs = lock(&self.jobs);
        if jobs.values().filter(|j| lock(&j.done).is_none()).count() >= 16 {
            return Err("active job limit".into());
        }
        if jobs
            .values()
            .any(|j| lock(&j.done).is_none() && (command == "quick" || j.command == "quick"))
        {
            return Err("device busy with another job".into());
        }
        if device.as_ref().is_some_and(|d| {
            jobs.values()
                .any(|j| j.device.as_ref() == Some(d) && lock(&j.done).is_none())
        }) {
            return Err("device busy with another job".into());
        }
        while jobs.len() >= 32 {
            let old = jobs
                .iter()
                .find(|(_, j)| lock(&j.done).is_some())
                .map(|(id, _)| id.clone());
            if let Some(old) = old {
                jobs.remove(&old);
            } else {
                break;
            }
        }
        let id = format!(
            "{}-{}",
            self.instance,
            self.serial.fetch_add(1, Ordering::Relaxed)
        );
        let job = Arc::new(Job {
            id: id.clone(),
            run_id: run.into(),
            command: command.into(),
            device,
            lease: Mutex::new(Instant::now() + ttl),
            stop: AtomicBool::new(false),
            overflow: Arc::new(AtomicBool::new(false)),
            stdout: Arc::new(Mutex::new(vec![])),
            stderr: Arc::new(Mutex::new(vec![])),
            done: Mutex::new(None),
        });
        let child = Command::new(&self.binary)
            .env("VALIDATOR_RUN_ID", run)
            .arg(command)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        jobs.insert(id.clone(), job.clone());
        thread::spawn(move || monitor(child, job));
        Ok(json::object(&[("id", text(id))]))
    }
    fn route(&self, method: &str, path: &str, body: &str) -> Result<V> {
        if method == "GET" && path == "/v1/health" {
            return Ok(json::object(&[
                ("api", V::Int(1)),
                ("instance", text(&self.instance)),
                ("version", text(env!("CARGO_PKG_VERSION"))),
            ]));
        }
        if method == "GET" && path == "/v1/topology" {
            return self.inspect("topology", vec![]);
        }
        if method == "GET" && path == "/v1/devices" {
            return self.inspect("list", vec![]);
        }
        if method == "POST" && path == "/v1/plan" {
            let v = json::parse(body)?;
            let args = args(&v)?;
            checked_args("plan", &args)?;
            return self.inspect("plan", args);
        }
        if method == "POST" && path == "/v1/jobs" {
            return self.start(&json::parse(body)?);
        }
        if method == "GET" && path == "/v1/jobs" {
            return Ok(V::Array(
                lock(&self.jobs).values().map(|j| j.value()).collect(),
            ));
        }
        if let Some(tail) = path.strip_prefix("/v1/jobs/") {
            let (id, action) = tail.split_once('/').unwrap_or((tail, ""));
            let job = lock(&self.jobs).get(id).cloned().ok_or("job not found")?;
            if method == "GET" && action.is_empty() {
                return Ok(job.value());
            }
            if method == "POST" && action == "heartbeat" {
                let ttl = lease(&json::parse(body)?)?;
                if lock(&job.done).is_none() {
                    let mut deadline = lock(&job.lease);
                    if Instant::now() >= *deadline {
                        return Err("job lease expired".into());
                    }
                    *deadline = Instant::now() + ttl;
                }
                return Ok(job.value());
            }
            if method == "POST" && action == "stop" {
                job.stop.store(true, Ordering::Relaxed);
                return Ok(job.value());
            }
        }
        Err("route not found".into())
    }
    fn inspect(&self, command: &str, args: Vec<String>) -> Result<V> {
        let mut child = Command::new(&self.binary)
            .arg(command)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = Arc::new(Mutex::new(vec![]));
        let stderr = Arc::new(Mutex::new(vec![]));
        let overflow = Arc::new(AtomicBool::new(false));
        let out = capture(
            child.stdout.take().ok_or("stdout missing")?,
            stdout.clone(),
            overflow.clone(),
        );
        let err = capture(
            child.stderr.take().ok_or("stderr missing")?,
            stderr.clone(),
            overflow.clone(),
        );
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if start.elapsed() > Duration::from_secs(10) || overflow.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out.join();
                let _ = err.join();
                return Err("inspection exceeded limits".into());
            }
            thread::sleep(Duration::from_millis(25));
        };
        let _ = out.join();
        let _ = err.join();
        let stdout = String::from_utf8_lossy(&lock(&stdout)).into_owned();
        let stderr = String::from_utf8_lossy(&lock(&stderr)).into_owned();
        Ok(json::object(&[
            (
                "exit_code",
                status.code().map_or(V::Null, |n| V::Int(i64::from(n))),
            ),
            ("reason", text("completed")),
            ("stdout", text(stdout)),
            ("stderr", text(stderr)),
        ]))
    }
}
struct Message {
    head: String,
    body: String,
}
fn read_message(stream: &mut TcpStream, max: usize) -> Result<Message> {
    let mut bytes = vec![];
    let end = loop {
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            if i + 4 > 16384 {
                return Err("header limit".into());
            }
            break i + 4;
        }
        if bytes.len() > 16384 {
            return Err("header limit".into());
        }
        let mut block = [0; 4096];
        let n = stream.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("truncated HTTP headers".into());
        }
        bytes.extend_from_slice(&block[..n]);
    };
    let head = std::str::from_utf8(&bytes[..end])
        .map_err(|e| e.to_string())?
        .to_string();
    let mut length = None;
    for line in head.split("\r\n").skip(1) {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("transfer-encoding") {
                return Err("chunked encoding is not supported".into());
            }
            if key.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate content length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "invalid content length")?,
                );
            }
        }
    }
    let length = length.unwrap_or(0);
    if length > max {
        return Err("body limit".into());
    }
    while bytes.len() - end < length {
        let mut block = [0; 8192];
        let n = stream.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("truncated HTTP body".into());
        }
        bytes.extend_from_slice(&block[..n]);
    }
    Ok(Message {
        head,
        body: String::from_utf8(bytes[end..end + length].to_vec()).map_err(|e| e.to_string())?,
    })
}
fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let _=write!(stream,"HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
}
pub fn serve(o: &Options) -> Result<bool> {
    let listener =
        TcpListener::bind(o.get("--listen", "0.0.0.0:5040")).map_err(|e| e.to_string())?;
    let token = o
        .values
        .get("--token")
        .cloned()
        .or_else(|| std::env::var("VALIDATOR_HTTP_TOKEN").ok())
        .filter(|t| !t.is_empty());
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let server = Arc::new(Server {
        jobs: Mutex::new(BTreeMap::new()),
        token,
        serial: AtomicUsize::new(1),
        instance: id(),
        binary: std::env::current_exe().map_err(|e| e.to_string())?,
    });
    let connections = Arc::new(AtomicUsize::new(0));
    eprintln!(
        "validator HTTP agent listening on {}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    while !crate::stop_requested() {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if connections.fetch_add(1, Ordering::Relaxed) >= 32 {
                    connections.fetch_sub(1, Ordering::Relaxed);
                    reply(&mut stream, 503, "{\"error\":\"connection limit\"}");
                    continue;
                }
                let server = server.clone();
                let count = connections.clone();
                thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let result = read_message(&mut stream, BODY_LIMIT).and_then(|m| {
                        let mut first =
                            m.head.lines().next().unwrap_or_default().split_whitespace();
                        let method = first.next().ok_or("missing method")?;
                        let path = first.next().ok_or("missing path")?;
                        if let Some(token) = &server.token {
                            let expected = format!("Bearer {token}");
                            let valid = m.head.split("\r\n").filter_map(|l| l.split_once(':')).any(
                                |(k, v)| {
                                    k.eq_ignore_ascii_case("authorization") && v.trim() == expected
                                },
                            );
                            if !valid {
                                return Err("unauthorized".into());
                            }
                        }
                        server.route(method, path, &m.body)
                    });
                    match result {
                        Ok(v) => reply(&mut stream, 200, &v.encode()),
                        Err(e) => reply(
                            &mut stream,
                            if e == "unauthorized" {
                                401
                            } else if e == "job not found" || e == "route not found" {
                                404
                            } else if e.contains("busy") {
                                409
                            } else {
                                400
                            },
                            &json::object(&[("error", text(e))]).encode(),
                        ),
                    }
                    count.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25))
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let jobs: Vec<_> = lock(&server.jobs).values().cloned().collect();
    for job in &jobs {
        job.stop.store(true, Ordering::Relaxed);
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while jobs.iter().any(|j| lock(&j.done).is_none()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    Ok(true)
}
#[derive(Clone)]
pub struct Endpoint {
    address: String,
    token: Option<String>,
}
impl Endpoint {
    pub fn new(url: &str, token: Option<String>) -> Result<Self> {
        let address = url
            .strip_prefix("http://")
            .ok_or("agent URL must use http://")?
            .trim_end_matches('/');
        if address.is_empty()
            || address.contains(char::is_whitespace)
            || address.contains(['/', '@', '?', '#'])
        {
            return Err("agent URL must be an HTTP origin".into());
        }
        let address = if address.starts_with('[') {
            if address.ends_with(']') {
                format!("{address}:80")
            } else {
                address.into()
            }
        } else if address.contains(':') {
            address.into()
        } else {
            format!("{address}:80")
        };
        Ok(Self { address, token })
    }
    pub fn request(&self, method: &str, path: &str, body: V) -> Result<V> {
        let mut stream = None;
        for addr in self.address.to_socket_addrs().map_err(|e| e.to_string())? {
            if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
                stream = Some(s);
                break;
            }
        }
        let mut stream = stream.ok_or("could not connect to HTTP agent")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let body = body.encode();
        let auth = self
            .token
            .as_ref()
            .map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        if auth.contains('\n') && !auth.ends_with("\r\n") {
            return Err("invalid token".into());
        }
        if self
            .token
            .as_ref()
            .is_some_and(|t| t.contains(['\r', '\n']))
        {
            return Err("invalid token".into());
        }
        write!(stream,"{method} {path} HTTP/1.1\r\nHost: {}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",self.address,body.len()).map_err(|e|e.to_string())?;
        let message = read_message(&mut stream, 12 * OUTPUT_LIMIT + 65536)?;
        let value = json::parse(&message.body)?;
        if message.head.split_whitespace().nth(1) != Some("200") {
            return Err(format!("HTTP agent: {}", value.get("error")?.string()?));
        }
        Ok(value)
    }
}
pub struct HttpJob {
    endpoint: Endpoint,
    id: String,
}
impl HttpJob {
    pub fn start(endpoint: Endpoint, args: &[String]) -> Result<Self> {
        let command = args.first().ok_or("empty command")?;
        let value = endpoint.request(
            "POST",
            "/v1/jobs",
            json::object(&[
                ("command", text(command)),
                ("args", V::Array(args[1..].iter().map(text).collect())),
                ("lease_secs", V::Int(15)),
                ("run_id", text(crate::run_id())),
            ]),
        )?;
        let id = value.get("id")?.string()?;
        if id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("invalid agent job ID".into());
        }
        Ok(Self {
            endpoint,
            id: id.into(),
        })
    }
    pub fn poll(&self) -> Result<Option<Output>> {
        let v = self
            .endpoint
            .request("GET", &format!("/v1/jobs/{}", self.id), V::Null)?;
        if v.get("running")?.boolean()? {
            Ok(None)
        } else {
            Output::from_value(&v).map(Some)
        }
    }
    pub fn heartbeat(&self) -> Result<()> {
        self.endpoint.request(
            "POST",
            &format!("/v1/jobs/{}/heartbeat", self.id),
            json::object(&[("lease_secs", V::Int(15))]),
        )?;
        Ok(())
    }
    pub fn stop(&self) -> Result<()> {
        self.endpoint
            .request("POST", &format!("/v1/jobs/{}/stop", self.id), V::Null)?;
        Ok(())
    }
}
impl Drop for HttpJob {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
pub fn endpoint(o: &Options, host_key: &str) -> Result<Option<Endpoint>> {
    let key = if host_key == "--tx-host" {
        "--tx-url"
    } else {
        "--rx-url"
    };
    o.values
        .get(key)
        .map(|url| {
            Endpoint::new(
                url,
                o.values
                    .get("--agent-token")
                    .cloned()
                    .or_else(|| std::env::var("VALIDATOR_HTTP_TOKEN").ok()),
            )
        })
        .transpose()
}
pub fn inspect(endpoint: Endpoint, command: &str, args: &[String]) -> Result<Output> {
    let v = if command == "list" {
        endpoint.request("GET", "/v1/devices", V::Null)?
    } else {
        endpoint.request(
            "POST",
            "/v1/plan",
            json::object(&[("args", V::Array(args.iter().map(text).collect()))]),
        )?
    };
    Output::from_value(&v)
}
