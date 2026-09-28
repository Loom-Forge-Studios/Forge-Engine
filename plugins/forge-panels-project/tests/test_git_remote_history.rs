// timed-gates: exempt(a 180 s deadline on a git subprocess, a hang guard; no time is asserted)
//! The WP-U7 revision-history and launcher panels over the **real Git backend** (WP-16,
//! M2-15), end to end headless: a project whose own history is Git (`git:`), GitHub sign-in
//! with the device flow from the panel, "Create on GitHub" linking the new repository, push
//! over Git's smart HTTP protocol, a second editor cloning it from the launcher, pull, and
//! divergence refused.
//!
//! One loopback server plays GitHub: its OAuth device-flow and REST endpoints (a mock), and
//! the repository itself — real `git upload-pack` / `git receive-pack` behind a smart-HTTP
//! front, requiring the token the device flow issued.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use forge_cmd::{EditorCommand, Issuer, Value};
use forge_editor::client::BusClient;
use forge_editor::core::{CoreFaults, EditorCore, SharedCore};
use forge_editor::project::{ProjectOp, Template, link_remote_command};
use forge_editor::testing::Rig;
use forge_project::host::ProjectHost;
use forge_store_backends::git::github::GitHub;
use forge_store_backends::git::{
    Credential, Credentials, GitOptions, HttpClient, HttpRequest, HttpResponse, UreqClient,
};

const H: &str = "forge.history";
const L: &str = "forge.launcher";
const TOKEN: &str = "gho_panel_token";

fn git(dir: &Path, args: &[&str]) -> String {
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
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
    let n = headers
        .iter()
        .find(|(h, _)| h.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= c.len() {
                char::from(T[((n >> (18 - 6 * i)) & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
}

fn git_service(repo: &Path, service: &str, advertise: bool, input: &[u8]) -> Vec<u8> {
    let mut cmd = Command::new("git");
    cmd.arg(service.trim_start_matches("git-"))
        .arg("--stateless-rpc");
    if advertise {
        cmd.arg("--advertise-refs");
    }
    let mut child = cmd
        .arg(repo)
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
}

/// GitHub on loopback: the device flow (pending once, then approved), the user, repository
/// creation, and the repository `/ada/orbits.git` served by real git.
fn serve_github(repo: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let polls = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (repo, polls) = (repo.clone(), Arc::clone(&polls));
            std::thread::spawn(move || {
                let Some(req) = read_req(&mut stream) else {
                    return;
                };
                let json = |status: u16, v: serde_json::Value| {
                    (
                        status,
                        "application/json".to_string(),
                        v.to_string().into_bytes(),
                    )
                };
                let bearer = req.header("authorization") == Some(&format!("Bearer {TOKEN}")[..]);
                let basic = req.header("authorization")
                    == Some(
                        &format!(
                            "Basic {}",
                            b64(format!("x-access-token:{TOKEN}").as_bytes())
                        )[..],
                    );
                let (status, ctype, body) = match (req.method.as_str(), req.path.as_str()) {
                    ("POST", "/login/device/code") => json(
                        200,
                        serde_json::json!({
                            "device_code": "dc-panel", "user_code": "WDJB-MJHT",
                            "verification_uri": "https://github.com/login/device",
                            "expires_in": 900, "interval": 5,
                        }),
                    ),
                    ("POST", "/login/oauth/access_token") => {
                        if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                            json(200, serde_json::json!({"error": "authorization_pending"}))
                        } else {
                            json(
                                200,
                                serde_json::json!({"access_token": TOKEN, "token_type": "bearer"}),
                            )
                        }
                    }
                    ("GET", "/user") if bearer => json(200, serde_json::json!({"login": "ada"})),
                    ("POST", "/user/repos") if bearer => {
                        let b: serde_json::Value =
                            serde_json::from_slice(&req.body).unwrap_or_default();
                        json(
                            201,
                            serde_json::json!({
                                "full_name": format!("ada/{}", b["name"].as_str().unwrap_or("")),
                                "clone_url": format!("http://127.0.0.1:{port}/ada/{}.git", b["name"].as_str().unwrap_or("")),
                                "html_url": "https://github.com/ada/orbits",
                            }),
                        )
                    }
                    (_, p) if p.starts_with("/ada/orbits.git/") => {
                        if !basic {
                            (401, "text/plain".to_string(), b"sign in".to_vec())
                        } else if let Some((_, svc)) = p.split_once("/info/refs?service=") {
                            let mut body =
                                format!("{:04x}# service={svc}\n", svc.len() + 15).into_bytes();
                            body.extend_from_slice(b"0000");
                            body.extend_from_slice(&git_service(&repo, svc, true, b""));
                            (200, format!("application/x-{svc}-advertisement"), body)
                        } else {
                            let svc = p.rsplit('/').next().unwrap_or("").to_string();
                            let out = git_service(&repo, &svc, false, &req.body);
                            (200, format!("application/x-{svc}-result"), out)
                        }
                    }
                    _ => json(404, serde_json::json!({"message": "Not Found"})),
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            });
        }
    });
    port
}

fn host(dir: &Path, creds: &Credentials, port: u16) -> ProjectHost {
    let opts = GitOptions {
        cache_dir: dir.join("git-cache"),
        credentials: creds.clone(),
        http: Arc::new(UreqClient::new()),
    };
    let mut h = ProjectHost::first_party_with(opts).unwrap_or_else(|e| panic!("{e}"));
    h.set_github(Some(GitHub {
        web: format!("http://127.0.0.1:{port}"),
        api: format!("http://127.0.0.1:{port}"),
        client_id: "Iv1.panel".into(),
        http: Arc::new(UreqClient::new()),
    }));
    h
}

fn rig(panels: &[&str], projects: &Path, host: ProjectHost) -> Rig {
    let cfg = common::config("3d");
    cfg.services.launcher.borrow_mut().projects_dir = projects.to_path_buf();
    let core = EditorCore::with_host(EditorCore::editor_bus(), host);
    let mut rig = Rig::with_core(cfg, core).unwrap_or_else(|e| panic!("{e}"));
    rig.show_panels(panels).unwrap_or_else(|e| panic!("{e}"));
    rig
}

fn press(rig: &mut Rig, panel: &str, path: &[&str]) {
    let id = common::part(rig, panel, path);
    common::click(rig, id);
}

fn label(rig: &Rig, panel: &str, path: &[&str]) -> String {
    let id = common::part(rig, panel, path);
    common::label(rig, id)
}

#[test]
fn history_signs_in_to_github_creates_the_repo_pushes_and_a_second_editor_clones_and_pulls() {
    let dir = common::tmp("git-remote-history");
    let bare = dir.join("github").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let port = serve_github(bare.clone());

    // Ada: a project whose own history is a Git repository.
    let ada_creds = Credentials::default();
    let mut ada = rig(&[H], &dir, host(&dir.join("ada-host"), &ada_creds, port));
    let project_dir = dir.join("ada").join("orbits");
    ada.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("git:{}", project_dir.display()),
            name: "Orbits".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    ada.settle();
    assert!(
        common::status(&ada)
            .open
            .as_ref()
            .is_some_and(|o| o.backend == "git")
    );
    ada.shell.emitter().emit(EditorCommand::Spawn {
        name: "Beacon".into(),
        parent: None,
    });
    ada.settle();
    press(&mut ada, H, &["content", "save_bar", "save"]);
    git(
        &project_dir.join(".forge").join("git"),
        &["fsck", "--strict", "--no-dangling"],
    );

    // Sign in from the panel: the code to enter is shown; Continue until approved.
    assert_eq!(
        label(&ada, H, &["content", "github", "github_state"]),
        "Not signed in."
    );
    press(&mut ada, H, &["content", "github", "github_bar", "sign_in"]);
    assert_eq!(
        label(&ada, H, &["content", "github", "github_state"]),
        "Enter WDJB-MJHT at https://github.com/login/device, then press Continue."
    );
    press(
        &mut ada,
        H,
        &["content", "github", "github_bar", "sign_in_continue"],
    );
    assert!(
        label(&ada, H, &["content", "status"]).contains("Waiting for GitHub"),
        "{}",
        label(&ada, H, &["content", "status"])
    );
    press(
        &mut ada,
        H,
        &["content", "github", "github_bar", "sign_in_continue"],
    );
    assert_eq!(
        label(&ada, H, &["content", "github", "github_state"]),
        format!("Signed in: ada on 127.0.0.1:{port}.")
    );
    assert!(
        ada_creds
            .for_url(&format!("http://127.0.0.1:{port}/x"))
            .is_some()
    );
    // The token is nowhere a client can see.
    let shown = format!("{:?}", common::status(&ada));
    assert!(!shown.contains(TOKEN), "the status never carries the token");

    // Create on GitHub: a private repository named after the project, linked as the remote.
    press(
        &mut ada,
        H,
        &["content", "github", "github_bar", "create_remote"],
    );
    let url = format!("http://127.0.0.1:{port}/ada/orbits.git");
    assert_eq!(
        common::setting(&ada, "project.remote"),
        Some(Value::Text(url.clone())),
        "{}",
        label(&ada, H, &["content", "status"])
    );
    press(&mut ada, H, &["content", "save_bar", "save"]);
    press(
        &mut ada,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );
    assert!(
        label(&ada, H, &["content", "status"]).contains("Pushed 2 revision(s)"),
        "{}",
        label(&ada, H, &["content", "status"])
    );
    git(&bare, &["fsck", "--strict", "--no-dangling"]);
    assert_eq!(
        git(&bare, &["rev-list", "--count", "refs/heads/main"]).trim(),
        "2"
    );
    // The same revisions are the same Git commits on the host and in the project folder.
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main"]),
        git(
            &project_dir.join(".forge").join("git"),
            &["rev-parse", "refs/heads/main"]
        )
    );

    // Bob (signed in on his machine) clones from the launcher by pasting the URL.
    let bob_creds = Credentials::default();
    bob_creds.set(&format!("127.0.0.1:{port}"), Credential::token(TOKEN));
    let bob_dir = dir.join("bob");
    let mut bob = rig(
        &[L, H],
        &bob_dir,
        host(&dir.join("bob-host"), &bob_creds, port),
    );
    let field = common::part(&bob, L, &["content", "clone_bar", "clone_url"]);
    common::type_into(&mut bob, field, &url);
    press(&mut bob, L, &["content", "clone_bar", "clone"]);
    let bst = common::status(&bob);
    let bopen = bst
        .open
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", bst.outcomes));
    assert_eq!(bopen.revisions.len(), 2);
    assert_eq!(
        bopen.head,
        common::status(&ada)
            .open
            .as_ref()
            .and_then(|o| o.head.clone()),
        "the same revision ids on both sides"
    );
    assert!(
        bob_dir.join("orbits").join("forge-project.ron").is_file(),
        "named after the repository"
    );
    assert!(common::names(&bob).contains(&"Beacon".to_string()));

    // Bob works and pushes; Ada pulls it back.
    bob.shell.emitter().emit(EditorCommand::Spawn {
        name: "Bob's moon".into(),
        parent: None,
    });
    bob.settle();
    press(&mut bob, H, &["content", "save_bar", "save"]);
    press(
        &mut bob,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );
    assert!(
        label(&bob, H, &["content", "status"]).contains("Pushed 1 revision(s)"),
        "{}",
        label(&bob, H, &["content", "status"])
    );
    press(
        &mut ada,
        H,
        &["content", "remote_group", "remote_bar", "pull"],
    );
    assert!(
        label(&ada, H, &["content", "status"]).contains("Pulled 1 revision(s)"),
        "{}",
        label(&ada, H, &["content", "status"])
    );
    assert!(common::names(&ada).contains(&"Bob's moon".to_string()));
    assert!(ada.mirror_matches());

    // Both save on the same head: the second push is refused, nothing is merged or lost.
    ada.shell.emitter().emit(EditorCommand::Spawn {
        name: "Ada's".into(),
        parent: None,
    });
    ada.settle();
    press(&mut ada, H, &["content", "save_bar", "save"]);
    bob.shell.emitter().emit(EditorCommand::Spawn {
        name: "Bob's".into(),
        parent: None,
    });
    bob.settle();
    press(&mut bob, H, &["content", "save_bar", "save"]);
    press(
        &mut bob,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );
    let before = git(&bare, &["rev-parse", "refs/heads/main"]);
    press(
        &mut ada,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );
    assert!(
        label(&ada, H, &["content", "status"]).contains("PROJECT-0007"),
        "{}",
        label(&ada, H, &["content", "status"])
    );
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_an_oauth_app_sign_in_says_so() {
    let dir = common::tmp("no-oauth-app");
    let opts = GitOptions {
        cache_dir: dir.join("git-cache"),
        ..GitOptions::new()
    };
    let mut h = ProjectHost::first_party_with(opts).unwrap_or_else(|e| panic!("{e}"));
    h.set_github(None);
    let mut r = rig(&[H], &dir, h);
    press(&mut r, H, &["content", "github", "github_bar", "sign_in"]);
    let s = label(&r, H, &["content", "status"]);
    assert!(
        s.contains("PROJECT-0011") && s.contains("FORGE_GITHUB_CLIENT_ID"),
        "{s}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- transfers run off the core's lock (owner rule 1) ------------------------------------

/// An HTTP client that, when armed, holds **every** request until the test lets it go, one
/// at a time: a slow network under the test's control. Each held request is announced as
/// `"<GET|POST> <url>"`, so a guard sees every phase of a transfer — the opener's fetch,
/// a push's `git-receive-pack` upload, a pull's or clone's `git-upload-pack` download,
/// GitHub's API — and not only the first request.
struct GatedHttp {
    inner: UreqClient,
    armed: AtomicBool,
    gate: Mutex<Option<Gate>>,
    /// Harness control only: the next held request waits **inside the core's lock**, as a
    /// transfer whose opener fetched under `lock(core)` would.
    lock_next: Mutex<Option<SharedCore>>,
}

/// Where a held request is announced, and where it waits to be let go.
#[derive(Clone)]
struct Gate {
    entered: mpsc::Sender<String>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl GatedHttp {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: UreqClient::new(),
            armed: AtomicBool::new(false),
            gate: Mutex::new(None),
            lock_next: Mutex::new(None),
        })
    }

    /// The next held request waits while holding `core`'s lock (the harness control).
    fn hold_core_lock_at_next_request(&self, core: &SharedCore) {
        *self
            .lock_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(core.clone());
    }

    /// Hold every request from now on: `(each request being held, let one go)`.
    fn arm(&self) -> (mpsc::Receiver<String>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *self.gate.lock().unwrap_or_else(PoisonError::into_inner) = Some(Gate {
            entered: entered_tx,
            release: Arc::new(Mutex::new(release_rx)),
        });
        self.armed.store(true, Ordering::SeqCst);
        (entered_rx, release_tx)
    }

    /// Let requests through unheld again.
    fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
        *self.gate.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl HttpClient for GatedHttp {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, String> {
        let gate = if self.armed.load(Ordering::SeqCst) {
            self.gate
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        } else {
            None
        };
        if let Some(gate) = gate {
            let what = format!("{} {}", if req.post { "POST" } else { "GET" }, req.url);
            let wait = || {
                if gate.entered.send(what).is_ok() {
                    let release = gate.release.lock().unwrap_or_else(PoisonError::into_inner);
                    // A broken test must not hang the suite: the network answers at last.
                    let _ = release.recv_timeout(Duration::from_secs(60));
                }
            };
            let lock_core = self
                .lock_next
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            match lock_core {
                Some(core) => EditorCore::read(&core, |_| wait()),
                None => wait(),
            }
        }
        self.inner.send(req)
    }
}

fn gated_host(dir: &Path, creds: &Credentials, port: u16, http: &Arc<GatedHttp>) -> ProjectHost {
    let http: Arc<dyn HttpClient> = http.clone();
    let opts = GitOptions {
        cache_dir: dir.join("git-cache"),
        credentials: creds.clone(),
        http: Arc::clone(&http),
    };
    let mut h = ProjectHost::first_party_with(opts).unwrap_or_else(|e| panic!("{e}"));
    h.set_github(Some(GitHub {
        web: format!("http://127.0.0.1:{port}"),
        api: format!("http://127.0.0.1:{port}"),
        client_id: "Iv1.panel".into(),
        http,
    }));
    h
}

/// Another client of `core` — an automation session — connects, previews a command, reads the
/// project and its status, and applies an edit when `edit`. The transfer the status names, or `Err`
/// when it could not do that within `probe`: the core's lock was held.
fn another_client_works(
    core: &SharedCore,
    edit: bool,
    probe: Duration,
) -> Result<Option<String>, String> {
    let core = core.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut auto = EditorCore::connect(
            &core,
            Issuer::Automation {
                session: "transfer-probe".into(),
                tool: "apply".into(),
            },
        );
        let spawn = EditorCommand::Spawn {
            name: "Made during a transfer".into(),
            parent: None,
        };
        let planned = auto.preview(&spawn).is_ok();
        if edit {
            auto.apply(spawn, None);
            let _ = auto.pump();
        }
        let _ = EditorCore::read(&core, |p| p.len());
        let st = EditorCore::project_status(&core);
        let _ = tx.send((planned, st.transfer.as_ref().map(|t| t.what.clone())));
    });
    match rx.recv_timeout(probe) {
        Ok((true, transfer)) => Ok(transfer),
        Ok((false, _)) => Err("the session's preview was refused".into()),
        Err(_) => Err(format!(
            "another client could not reach the core for {:.1} s: its lock is held",
            probe.as_secs_f64()
        )),
    }
}

/// How long the guard gives another client to connect, preview, read and edit while a
/// request is held: that work takes milliseconds, so running out means the lock is held.
const PROBE: Duration = Duration::from_secs(10);

/// The positive controls' probe: they wait out one probe they expect to fail, so theirs is
/// shorter (a working probe still finishes in milliseconds).
const CONTROL_PROBE: Duration = Duration::from_secs(3);

/// What a request after the one that found the lock held records: once caught, the rest of
/// the transfer is let through unprobed (each probe would wait out `PROBE` again).
const NOT_PROBED: &str = "not probed: an earlier request of this transfer held the core's lock";

/// Whether `core` still runs a transfer, asked on a thread of its own (the ask takes the
/// core's lock, which a faulty transfer may hold while its request waits on the test).
fn ask_running(core: &SharedCore) -> mpsc::Receiver<bool> {
    let (tx, rx) = mpsc::channel();
    let core = core.clone();
    std::thread::spawn(move || {
        let _ = tx.send(EditorCore::project_status(&core).transfer.is_some());
    });
    rx
}

/// One held request of a transfer and what another client managed while it was held.
type HeldRequest = (String, Result<Option<String>, String>);

/// Drive the transfer `core` has started, whose requests `http` (armed: `entered`,
/// `release`) holds: at **each** request another client works (edits when `edit`; `probe`
/// bounds it), then — at the first request only, and only when that client got through —
/// `at_first`, then the request goes. Returns every request held, in order, with what the
/// other client managed; the transfer has ended and `http` is disarmed.
///
/// The probe runs before `at_first` because `at_first` drives the editor, which takes the
/// core's lock: a transfer holding that lock at its first request would block it until the
/// network gave up, and the guard would time out on a transfer that "never ended" instead
/// of naming the held lock. Once a probe finds the lock held, the transfer's remaining
/// requests go through unprobed ([`NOT_PROBED`]), so a caught transfer ends at once.
fn hold_each_request(
    core: &SharedCore,
    http: &GatedHttp,
    (entered, release): (mpsc::Receiver<String>, mpsc::Sender<()>),
    edit: bool,
    probe: Duration,
    mut at_first: impl FnMut(&str),
) -> Vec<HeldRequest> {
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let mut held: Vec<HeldRequest> = Vec::new();
    loop {
        let next = if held.is_empty() {
            Some(
                entered
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap_or_else(|_| panic!("the transfer never reached the network")),
            )
        } else {
            // Its next request, or its end.
            let mut asked: Option<mpsc::Receiver<bool>> = None;
            loop {
                match entered.recv_timeout(Duration::from_millis(20)) {
                    Ok(r) => break Some(r),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let answer = asked.get_or_insert_with(|| ask_running(core)).try_recv();
                match answer {
                    Ok(false) => break entered.try_recv().ok(),
                    Ok(true) | Err(mpsc::TryRecvError::Disconnected) => asked = None,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the transfer never ended; held so far: {held:?}"
                );
            }
        };
        let Some(req) = next else { break };
        let caught = held.iter().any(|(_, w)| w.is_err());
        let worked = if caught {
            Err(NOT_PROBED.to_string())
        } else {
            another_client_works(core, edit, probe)
        };
        if held.is_empty() && worked.is_ok() {
            at_first(&req);
        }
        held.push((req, worked));
        let _ = release.send(());
    }
    http.disarm();
    held
}

/// Every request of `held` was held while another client worked, and the status named the
/// transfer (`shown` starts with it, when given). The requests, in order.
fn all_worked(what: &str, held: &[HeldRequest], shown: Option<&str>) -> Vec<String> {
    for (req, worked) in held {
        let running = worked
            .as_ref()
            .unwrap_or_else(|e| panic!("{what}: while `{req}` was held: {e}"));
        assert!(
            running.is_some(),
            "{what}: while `{req}` was held the status names no transfer"
        );
        if let (Some(w), Some(shown)) = (running.as_deref(), shown) {
            assert!(
                shown.starts_with(w),
                "{what}: the status names the transfer ({w}) and the panel shows it ({shown})"
            );
        }
    }
    held.iter().map(|(r, _)| r.clone()).collect()
}

/// Whether `requests` include a `POST` to `<repo>/<service>` (a Git transfer's data phase).
fn posted(requests: &[String], service: &str) -> bool {
    requests
        .iter()
        .any(|r| r.starts_with("POST ") && r.ends_with(&format!("/{service}")))
}

/// Press `path` on `panel` with the network held, request by request: while each request
/// waits, another client works (and edits when `edit`), and at the first the panel says
/// what is running; then the outcome arrives. The panel's status line after, and every
/// request the transfer made. `probe` bounds each other-client probe ([`PROBE`] in the
/// guard).
fn press_during_held_network(
    rig: &mut Rig,
    http: &GatedHttp,
    panel: &str,
    path: &[&str],
    edit: bool,
    probe: Duration,
) -> (String, Vec<String>) {
    let gate = http.arm();
    let id = common::part(rig, panel, path);
    // Clicked and routed, not settled: nothing in the test waits for the transfer.
    rig.h.click(id);
    rig.turn();
    let core = rig.core.clone();
    let mut shown = String::new();
    let held = hold_each_request(&core, http, gate, edit, probe, |_| {
        rig.turn();
        shown = label(rig, panel, &["content", "status"]);
    });
    // The lock first: when it was held at the first request, the panel was never asked.
    let requests = all_worked(&format!("{path:?}"), &held, Some(&shown));
    assert!(
        shown.ends_with('\u{2026}'),
        "{path:?}: the panel says the transfer is running: {shown}"
    );
    rig.settle();
    assert!(
        common::status(rig).transfer.is_none(),
        "{path:?}: the transfer ended"
    );
    (label(rig, panel, &["content", "status"]), requests)
}

#[test]
fn every_transfer_runs_off_the_core_lock_so_the_editor_and_sessions_keep_working() {
    let dir = common::tmp("git-transfers-off-lock");
    let bare = dir.join("github").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let port = serve_github(bare.clone());
    let http = GatedHttp::new();
    let ada_creds = Credentials::default();
    let mut ada = rig(
        &[H],
        &dir,
        gated_host(&dir.join("ada-host"), &ada_creds, port, &http),
    );
    let project_dir = dir.join("ada").join("orbits");
    ada.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("git:{}", project_dir.display()),
            name: "Orbits".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    ada.settle();
    press(&mut ada, H, &["content", "save_bar", "save"]);

    // Sign-in start: GitHub is slow to hand out the code.
    let (s, reqs) = press_during_held_network(
        &mut ada,
        &http,
        H,
        &["content", "github", "github_bar", "sign_in"],
        false,
        PROBE,
    );
    assert!(s.contains("WDJB-MJHT"), "{s}");
    assert!(
        reqs.iter()
            .any(|r| r.starts_with("POST ") && r.ends_with("/login/device/code")),
        "{reqs:?}"
    );
    for _ in 0..2 {
        press(
            &mut ada,
            H,
            &["content", "github", "github_bar", "sign_in_continue"],
        );
    }
    assert!(
        label(&ada, H, &["content", "status"]).contains("Signed in to GitHub as ada"),
        "{}",
        label(&ada, H, &["content", "status"])
    );

    // Create on GitHub, slowly; a second transfer meanwhile is refused at once, naming
    // what runs.
    let gate = http.arm();
    ada.shell.emitter().emit(
        ProjectOp::CreateRemote {
            name: "orbits".into(),
        }
        .command(),
    );
    ada.turn();
    let core = ada.core.clone();
    let mut busy = None;
    let held = hold_each_request(&core, &http, gate, true, PROBE, |_| {
        ada.shell.emitter().emit(ProjectOp::Push.command());
        ada.turn();
        busy = common::status(&ada)
            .outcomes
            .last()
            .map(|o| o.result.clone());
    });
    // The lock first: when it was held at the first request, the second push was never sent.
    let reqs = all_worked("create_remote", &held, None);
    assert!(
        matches!(&busy, Some(Err((code, msg)))
            if code == "PROJECT-0012" && msg.contains("Creating the GitHub repository orbits")),
        "{busy:?}"
    );
    assert!(
        reqs.iter()
            .any(|r| r.starts_with("POST ") && r.ends_with("/user/repos")),
        "{reqs:?}"
    );
    ada.settle();
    let url = format!("http://127.0.0.1:{port}/ada/orbits.git");
    assert_eq!(
        common::setting(&ada, "project.remote"),
        Some(Value::Text(url.clone()))
    );
    press(&mut ada, H, &["content", "save_bar", "save"]);

    // Push over smart HTTP, slowly, with an automation session editing at every request — the
    // upload (`git-receive-pack`) included.
    let (s, reqs) = press_during_held_network(
        &mut ada,
        &http,
        H,
        &["content", "remote_group", "remote_bar", "push"],
        true,
        PROBE,
    );
    assert!(s.contains("Pushed 2 revision(s)"), "{s}");
    assert!(posted(&reqs, "git-receive-pack"), "{reqs:?}");
    assert!(common::names(&ada).contains(&"Made during a transfer".to_string()));
    press(&mut ada, H, &["content", "save_bar", "save"]);
    press(
        &mut ada,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );
    assert!(
        label(&ada, H, &["content", "status"]).contains("Pushed 1 revision(s)"),
        "{}",
        label(&ada, H, &["content", "status"])
    );

    // Bob clones from the launcher over a slow network.
    let bob_creds = Credentials::default();
    bob_creds.set(&format!("127.0.0.1:{port}"), Credential::token(TOKEN));
    let bob_http = GatedHttp::new();
    let bob_dir = dir.join("bob");
    let mut bob = rig(
        &[L, H],
        &bob_dir,
        gated_host(&dir.join("bob-host"), &bob_creds, port, &bob_http),
    );
    let field = common::part(&bob, L, &["content", "clone_bar", "clone_url"]);
    common::type_into(&mut bob, field, &url);
    let (s, reqs) = press_during_held_network(
        &mut bob,
        &bob_http,
        L,
        &["content", "clone_bar", "clone"],
        false,
        PROBE,
    );
    assert!(s.contains("Cloned") && s.contains("3 revision(s)"), "{s}");
    assert!(posted(&reqs, "git-upload-pack"), "{reqs:?}");
    bob.shell.emitter().emit(EditorCommand::Spawn {
        name: "Bob's moon".into(),
        parent: None,
    });
    bob.settle();
    press(&mut bob, H, &["content", "save_bar", "save"]);
    press(
        &mut bob,
        H,
        &["content", "remote_group", "remote_bar", "push"],
    );

    // Ada pulls over a slow network (an automation session reads meanwhile; an edit would be
    // unsaved work the pull refuses to overwrite).
    let (s, reqs) = press_during_held_network(
        &mut ada,
        &http,
        H,
        &["content", "remote_group", "remote_bar", "pull"],
        false,
        PROBE,
    );
    assert!(s.contains("Pulled 1 revision(s)"), "{s}");
    assert!(posted(&reqs, "git-upload-pack"), "{reqs:?}");
    assert!(common::names(&ada).contains(&"Bob's moon".to_string()));
    assert!(ada.mirror_matches());
    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 positive control for the guard above: the real push with its upload moved under the
/// core's lock (`CoreFaults::push_publishes_under_lock` — one guard across the replay,
/// `publish` and recording the head) is caught at the `git-receive-pack` upload's first
/// request, while the opener's fetch before it, still off the lock, passes. Its probe is
/// [`CONTROL_PROBE`], so waiting out the one it expects to fail stays short.
#[test]
fn positive_control_a_push_that_uploads_under_the_core_lock_is_caught() {
    let dir = common::tmp("git-push-under-lock");
    let bare = dir.join("github").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let port = serve_github(bare);
    let http = GatedHttp::new();
    let creds = Credentials::default();
    creds.set(&format!("127.0.0.1:{port}"), Credential::token(TOKEN));
    let mut ada = rig(
        &[H],
        &dir,
        gated_host(&dir.join("ada-host"), &creds, port, &http),
    );
    ada.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("git:{}", dir.join("ada").join("orbits").display()),
            name: "Orbits".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    ada.settle();
    let url = format!("http://127.0.0.1:{port}/ada/orbits.git");
    ada.shell.emitter().emit(link_remote_command(&url));
    ada.settle();
    press(&mut ada, H, &["content", "save_bar", "save"]);

    EditorCore::set_faults(
        &ada.core,
        CoreFaults {
            push_publishes_under_lock: true,
            ..CoreFaults::default()
        },
    );
    let gate = http.arm();
    ada.shell.emitter().emit(ProjectOp::Push.command());
    ada.turn();
    let core = ada.core.clone();
    let held = hold_each_request(&core, &http, gate, false, CONTROL_PROBE, |_| {});
    ada.settle();
    let caught = |r: &HeldRequest| r.1.as_ref().is_err_and(|e| e.contains("its lock is held"));
    // Caught at the upload's first request (`git-receive-pack`: its advertisement, inside
    // the guard with the replay); the upload itself (the POST) then goes unprobed.
    let first_caught = held.iter().position(caught);
    assert!(
        first_caught.is_some_and(|i| held[i].0.contains("git-receive-pack")),
        "the probe caught the upload under the lock: {held:?}"
    );
    assert!(
        held.iter()
            .skip(first_caught.map_or(0, |i| i + 1))
            .all(|r| r.1.as_ref().is_err_and(|e| e == NOT_PROBED)),
        "once caught, the rest of the transfer goes through unprobed: {held:?}"
    );
    assert!(
        held.last()
            .is_some_and(|r| r.0.starts_with("POST ") && r.0.ends_with("/git-receive-pack")),
        "the upload ran after the catch: {held:?}"
    );
    assert!(
        held.first()
            .is_some_and(|r| r.0.contains("service=git-upload-pack") && !caught(r)),
        "the opener's fetch, off the lock, passes: {held:?}"
    );
    // The push itself went through: only where it ran was wrong.
    let pushed = common::status(&ada)
        .outcomes
        .last()
        .map(|o| o.result.clone());
    assert!(
        matches!(&pushed, Some(Ok(m)) if m.contains("Pushed")),
        "{pushed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// W2 positive control for the harness itself: a transfer that holds the core's lock at its
/// **first** request (here the network client takes the lock around the opener's fetch, as
/// an opener moved under `lock(core)` would) is named at once — "its lock is held", at that
/// request — within one probe, not after the panel's own turn waits out the held network
/// and the guard's deadline.
#[test]
fn positive_control_a_transfer_holding_the_core_lock_at_its_first_request_is_caught_fast() {
    let dir = common::tmp("git-first-request-under-lock");
    let bare = dir.join("github").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let port = serve_github(bare);
    let http = GatedHttp::new();
    let creds = Credentials::default();
    creds.set(&format!("127.0.0.1:{port}"), Credential::token(TOKEN));
    let mut ada = rig(
        &[H],
        &dir,
        gated_host(&dir.join("ada-host"), &creds, port, &http),
    );
    ada.shell.emitter().emit(
        ProjectOp::Create {
            location: format!("git:{}", dir.join("ada").join("orbits").display()),
            name: "Orbits".into(),
            template: Template::ThreeD,
            discard_unsaved: false,
        }
        .command(),
    );
    ada.settle();
    let url = format!("http://127.0.0.1:{port}/ada/orbits.git");
    ada.shell.emitter().emit(link_remote_command(&url));
    ada.settle();
    press(&mut ada, H, &["content", "save_bar", "save"]);

    http.hold_core_lock_at_next_request(&ada.core);
    let start = std::time::Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        press_during_held_network(
            &mut ada,
            &http,
            H,
            &["content", "remote_group", "remote_bar", "push"],
            false,
            CONTROL_PROBE,
        )
    }));
    let took = start.elapsed();
    let caught = match outcome {
        Ok(passed) => {
            panic!("a transfer holding the core's lock at its first request passed: {passed:?}")
        }
        Err(e) => e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default(),
    };
    assert!(
        caught.contains("service=git-upload-pack") && caught.contains("its lock is held"),
        "the guard names the held lock at the first request: {caught}"
    );
    assert!(
        took < Duration::from_secs(30),
        "caught within one probe, not the network's or the guard's timeout: {took:?}"
    );
    // The push itself went through once the lock was let go: only where it ran was wrong.
    ada.settle();
    let pushed = common::status(&ada)
        .outcomes
        .last()
        .map(|o| o.result.clone());
    assert!(
        matches!(&pushed, Some(Ok(m)) if m.contains("Pushed")),
        "{pushed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
