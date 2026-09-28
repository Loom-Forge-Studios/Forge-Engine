//! Shared test set-up: temporary folders, the `git` command line (the independent check
//! that what the Git store writes is a valid repository), and a small HTTP/1.1 server on
//! loopback for the mock GitHub and the smart-HTTP front over real `git`.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use forge_cmd::{
    Bus, Change, CommandEnvelope, CommandSink, EditorCommand, FixedClock, Issuer, Value,
};
use forge_store::{Blake3, Bytes, ProjectStore, Rev, RevId, RevRange, StoreError, StorePath};

pub fn p(s: &str) -> StorePath {
    StorePath::new(s).unwrap_or_else(|e| panic!("{e}"))
}

/// A fresh folder for one test.
pub fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("git-store")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{e}"));
    d
}

/// Run `git` (isolated from the user's configuration); stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap_or_else(|e| panic!("the git command line is needed for this check: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Real envelopes from a real bus: a human builds a scene, an automation session tweaks it.
pub fn envelopes() -> Vec<CommandEnvelope> {
    let mut bus = Bus::with_clock(Box::new(FixedClock(0)));
    let ada = Issuer::Human { user: "ada".into() };
    let auto = Issuer::Automation {
        session: "sess-4".into(),
        tool: "apply".into(),
    };
    let mut out = Vec::new();
    let e = bus.envelope(
        ada.clone(),
        EditorCommand::Spawn {
            name: "World".into(),
            parent: None,
        },
    );
    out.push(e.clone());
    let world = match bus.apply(e).map(|a| a.diff.changes.first().cloned()) {
        Ok(Some(Change::Created { entity, .. })) => entity,
        other => panic!("{other:?}"),
    };
    for (who, x) in [(ada, 1.0), (auto.clone(), 2.5), (auto, -0.0)] {
        let e = bus.envelope(
            who,
            EditorCommand::SetProperty {
                entity: world,
                path: "light.intensity".into(),
                value: Value::Float(x),
            },
        );
        out.push(e.clone());
        bus.apply(e).unwrap_or_else(|e| panic!("{e:?}"));
    }
    out
}

pub const ROCK: &[u8] = b"PNG\r\n\x1a\n\0\xffrock\r\n";

/// The parity script (tests/store/test_store_backend_parity.rs's, verbatim in effect).
pub fn script(s: &mut dyn ProjectStore) -> Result<Vec<RevId>, StoreError> {
    let envs = envelopes();
    s.write(
        &p("scenes/main.ron"),
        Bytes::from_static(b"Scene(\n  lights: 1,\n)\n"),
    )?;
    s.write(&p("assets/rock.png"), Bytes::from_static(ROCK))?;
    s.write(
        &p("project.ron"),
        Bytes::from_static(b"Project(preset: ThreeD)\r\n"),
    )?;
    let rock_h = s.blob_put(Bytes::from_static(ROCK))?;
    assert_eq!(rock_h, Blake3::of(ROCK));
    let r1 = s.commit("human:ada built the world", &envs[..2])?;
    s.lock(&p("assets/rock.png"))?;
    s.write(
        &p("scenes/main.ron"),
        Bytes::from_static(b"Scene(\n  lights: 2,\n)\n"),
    )?;
    s.write(&p("levels/one.ron"), Bytes::from_static(b"Level(1)"))?;
    s.delete(&p("project.ron"))?;
    let r2 = s.commit("automation:sess-4 tuned the light", &envs[2..])?;
    s.delete(&p("levels/one.ron"))?;
    s.write(&p("notes.txt"), Bytes::from_static(b"uncommitted\n"))?;
    Ok(vec![r1, r2])
}

/// Fast-forward `dst` from `src` through the trait (what `forge_project::sync` does), then
/// publish: the revisions `dst` lacks, oldest first.
pub fn push(src: &dyn ProjectStore, dst: &mut dyn ProjectStore) -> Result<Vec<RevId>, StoreError> {
    let mut revs: Vec<Rev> = src.history(RevRange {
        to: None,
        stop_at: dst.head()?,
        limit: None,
    })?;
    revs.reverse();
    let mut out = Vec::new();
    for r in &revs {
        let tree = src.tree(&r.id)?.entries;
        for q in dst.list()? {
            if !tree.iter().any(|(w, _)| *w == q) {
                dst.delete(&q)?;
            }
        }
        for (q, h) in &tree {
            if dst.read(q).map(|b| Blake3::of(&b) == *h).unwrap_or(false) {
                continue;
            }
            dst.write(q, src.blob_get(*h)?)?;
        }
        let id = dst.commit_at(&r.message, &src.commands(&r.id)?, r.at_ms)?;
        assert_eq!(id, r.id, "a replayed revision keeps its id");
        out.push(id);
    }
    if !out.is_empty() {
        dst.publish()?;
    }
    Ok(out)
}

// ---- a small HTTP/1.1 server -------------------------------------------------------------

/// A request the server received.
#[derive(Clone, Debug)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

/// A response to send.
pub struct Resp {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Resp {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            body: body.into(),
            headers: Vec::new(),
        }
    }
    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Self::new(status, "application/json", v.to_string())
    }
}

fn read_req(stream: &mut dyn Read) -> Option<Req> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.trim_end().split(' ');
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let get = |k: &str| {
        headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if let Some(n) = get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.resize(n, 0);
        r.read_exact(&mut body).ok()?;
    } else if get("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            r.read_line(&mut size).ok()?;
            let n = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            let mut chunk = vec![0u8; n + 2];
            r.read_exact(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

/// Serve `handler` on a loopback port (one request per connection); the port.
pub fn serve(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || {
                let Some(req) = read_req(&mut stream) else {
                    return;
                };
                let resp = handler(&req);
                let reason = match resp.status {
                    200 => "OK",
                    201 => "Created",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    422 => "Unprocessable Entity",
                    _ => "Status",
                };
                let mut head = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    resp.status,
                    resp.content_type,
                    resp.body.len()
                );
                for (k, v) in &resp.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&resp.body);
                let _ = stream.flush();
            });
        }
    });
    port
}

fn pkt_line(s: &str) -> Vec<u8> {
    let mut v = format!("{:04x}", s.len() + 4).into_bytes();
    v.extend_from_slice(s.as_bytes());
    v
}

/// What the smart-HTTP front did (for the tests' assertions).
#[derive(Default)]
pub struct Seen {
    pub requests: std::sync::Mutex<Vec<String>>,
}

/// A smart-HTTP front over real `git upload-pack` / `git receive-pack` (what
/// `git http-backend` does), serving the bare repository `repo` at `/<anything>.git`, and
/// requiring HTTP Basic `x-access-token:<token>` when `token` is set.
pub fn serve_git(repo: PathBuf, token: Option<String>, seen: Arc<Seen>) -> u16 {
    serve(move |req| {
        if let Ok(mut s) = seen.requests.lock() {
            s.push(format!("{} {}", req.method, req.path));
        }
        if let Some(t) = &token {
            let want = format!("Basic {}", b64(format!("x-access-token:{t}").as_bytes()));
            if req.header("authorization") != Some(want.as_str()) {
                let mut r = Resp::new(401, "text/plain", "sign in");
                r.headers
                    .push(("WWW-Authenticate".into(), "Basic realm=\"git\"".into()));
                return r;
            }
        }
        let run = |service: &str, advertise: bool, input: &[u8]| -> Vec<u8> {
            let mut cmd = Command::new("git");
            cmd.arg(service.trim_start_matches("git-"))
                .arg("--stateless-rpc");
            if advertise {
                cmd.arg("--advertise-refs");
            }
            let mut child = cmd
                .arg(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .env_remove("GIT_PROTOCOL")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap_or_else(|e| panic!("git: {e}"));
            if let Some(mut i) = child.stdin.take() {
                let _ = i.write_all(input);
            }
            child
                .wait_with_output()
                .map(|o| o.stdout)
                .unwrap_or_default()
        };
        if req.method == "GET" {
            let Some((_, q)) = req.path.split_once("/info/refs?service=") else {
                return Resp::new(404, "text/plain", "no");
            };
            let service = q.to_string();
            let mut body = pkt_line(&format!("# service={service}\n"));
            body.extend_from_slice(b"0000");
            body.extend_from_slice(&run(&service, true, b""));
            return Resp::new(200, &format!("application/x-{service}-advertisement"), body);
        }
        for service in ["git-upload-pack", "git-receive-pack"] {
            if req.path.ends_with(&format!("/{service}")) {
                let out = run(service, false, &req.body);
                return Resp::new(200, &format!("application/x-{service}-result"), out);
            }
        }
        Resp::new(404, "text/plain", "no")
    })
}

/// Standard base64 (for the expected Authorization header).
pub fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(char::from(T[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Whether `git` succeeds.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
