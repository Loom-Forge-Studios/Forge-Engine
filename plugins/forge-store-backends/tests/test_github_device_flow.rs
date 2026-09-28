//! GitHub sign-in with the OAuth device flow and "repo created for you" (Ch.33.5, M2-15),
//! against a mock of GitHub's endpoints on loopback, through the production HTTP client —
//! then the token signs a push to a smart-HTTP Git server that requires it.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{Req, Resp, Seen, git, push, script, serve, serve_git, tmp};
use forge_store::ProjectStore;
use forge_store_backends::git::github::{GitHub, Poll};
use forge_store_backends::git::{Credential, Credentials, GitOptions, GitStore, UreqClient};

fn form(req: &Req) -> Vec<(String, String)> {
    String::from_utf8_lossy(&req.body)
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%3A", ":")))
        .collect()
}

fn has(req: &Req, k: &str, v: &str) -> bool {
    form(req).iter().any(|(a, b)| a == k && b == v)
}

/// A mock of github.com's device-flow endpoints and api.github.com's user and repos.
fn mock_github(clone_url: String) -> (u16, Arc<Mutex<Vec<String>>>) {
    let polls = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = Arc::clone(&log);
    let created = Arc::new(AtomicUsize::new(0));
    let port = serve(move |req| {
        if let Ok(mut l) = log2.lock() {
            l.push(format!("{} {}", req.method, req.path));
        }
        let json_accept = req.header("accept").is_some_and(|a| a.contains("json"));
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/login/device/code") => {
                if !json_accept || !has(req, "client_id", "Iv1.test") || !has(req, "scope", "repo")
                {
                    return Resp::json(400, &serde_json::json!({"error": "bad request"}));
                }
                Resp::json(
                    200,
                    &serde_json::json!({
                        "device_code": "dc-1",
                        "user_code": "WDJB-MJHT",
                        "verification_uri": "https://github.com/login/device",
                        "expires_in": 900,
                        "interval": 5,
                    }),
                )
            }
            ("POST", "/login/oauth/access_token") => {
                if !has(
                    req,
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code",
                ) {
                    return Resp::json(
                        200,
                        &serde_json::json!({"error": "unsupported_grant_type"}),
                    );
                }
                if has(req, "device_code", "dc-denied") {
                    return Resp::json(200, &serde_json::json!({"error": "access_denied"}));
                }
                if has(req, "device_code", "dc-old") {
                    return Resp::json(200, &serde_json::json!({"error": "expired_token"}));
                }
                // Pending, then "slow down", then approved.
                match polls.fetch_add(1, Ordering::SeqCst) {
                    0 => Resp::json(200, &serde_json::json!({"error": "authorization_pending"})),
                    1 => Resp::json(
                        200,
                        &serde_json::json!({"error": "slow_down", "interval": 10}),
                    ),
                    _ => Resp::json(
                        200,
                        &serde_json::json!({
                            "access_token": "gho_devicetoken",
                            "token_type": "bearer",
                            "scope": "repo",
                        }),
                    ),
                }
            }
            ("GET", "/user") => {
                if req.header("authorization") != Some("Bearer gho_devicetoken") {
                    return Resp::json(401, &serde_json::json!({"message": "Bad credentials"}));
                }
                Resp::json(200, &serde_json::json!({"login": "ada"}))
            }
            ("POST", "/user/repos") => {
                if req.header("authorization") != Some("Bearer gho_devicetoken") {
                    return Resp::json(401, &serde_json::json!({"message": "Bad credentials"}));
                }
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
                if body["private"] != serde_json::Value::Bool(true) {
                    return Resp::json(400, &serde_json::json!({"message": "want private"}));
                }
                if created.fetch_add(1, Ordering::SeqCst) > 0 {
                    return Resp::json(
                        422,
                        &serde_json::json!({"message": "name already exists on this account"}),
                    );
                }
                Resp::json(
                    201,
                    &serde_json::json!({
                        "full_name": format!("ada/{}", body["name"].as_str().unwrap_or("")),
                        "clone_url": clone_url,
                        "html_url": "https://github.com/ada/orbits",
                    }),
                )
            }
            _ => Resp::json(404, &serde_json::json!({"message": "Not Found"})),
        }
    });
    (port, log)
}

#[test]
fn device_flow_signs_in_creates_the_repo_and_the_token_signs_the_push() {
    let d = tmp("device-flow");
    // The Git server the created repository lives on (it wants the device-flow token).
    let bare = d.join("server").join("orbits.git");
    std::fs::create_dir_all(&bare).unwrap_or_else(|e| panic!("{e}"));
    git(&bare, &["init", "--quiet", "--bare"]);
    let git_port = serve_git(
        bare.clone(),
        Some("gho_devicetoken".into()),
        Arc::new(Seen::default()),
    );
    let clone_url = format!("http://127.0.0.1:{git_port}/ada/orbits.git");
    let (port, log) = mock_github(clone_url.clone());
    let gh = GitHub {
        web: format!("http://127.0.0.1:{port}"),
        api: format!("http://127.0.0.1:{port}"),
        client_id: "Iv1.test".into(),
        http: Arc::new(UreqClient::new()),
    };

    let code = gh.start("repo").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(code.user_code, "WDJB-MJHT");
    assert_eq!(code.verification_uri, "https://github.com/login/device");
    assert_eq!(code.interval, 5);
    // The caller polls on its own schedule; each poll is one request.
    assert_eq!(gh.poll(&code).ok(), Some(Poll::Pending));
    assert_eq!(gh.poll(&code).ok(), Some(Poll::SlowDown(10)));
    let token = match gh.poll(&code) {
        Ok(Poll::Token(t)) => t,
        other => panic!("{other:?}"),
    };
    assert_eq!(gh.user(&token).ok().as_deref(), Some("ada"));
    assert!(matches!(
        gh.user("gho_wrong"),
        Err(forge_store::StoreError::Unauthorized { .. })
    ));

    // "Repo created for you": private, its clone URL is what the project links.
    let repo = gh
        .create_repo(&token, "orbits")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(repo.full_name, "ada/orbits");
    assert_eq!(repo.clone_url, clone_url);
    let taken = gh
        .create_repo(&token, "orbits")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        taken.contains("already exists") && taken.contains("link it"),
        "{taken}"
    );

    // The token goes into the credentials for the repository's host: the push is signed.
    let creds = Credentials::default();
    let host = forge_store_backends::git::host_of(&repo.clone_url).unwrap_or_default();
    creds.set(&host, Credential::token(&token));
    let opts = GitOptions {
        cache_dir: d.join("cache"),
        credentials: creds,
        ..GitOptions::new()
    };
    let mut ada = GitStore::open_folder(d.join("ada"), "ada").unwrap_or_else(|e| panic!("{e}"));
    let ids = script(&mut ada).unwrap_or_else(|e| panic!("{e}"));
    let mut remote =
        GitStore::open_remote(&repo.clone_url, "ada", &opts).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        push(&ada, &mut remote).unwrap_or_else(|e| panic!("{e}")),
        ids
    );
    assert_eq!(remote.head().ok().flatten(), ids.last().copied());
    git(&bare, &["fsck", "--strict", "--no-dangling"]);

    // The token never shows in what the mock saw in a URL.
    let seen = log.lock().map(|l| l.clone()).unwrap_or_default();
    assert!(seen.iter().all(|l| !l.contains("gho_")), "{seen:?}");
}

#[test]
fn a_declined_or_expired_sign_in_is_named() {
    let (port, _) = mock_github(String::new());
    let gh = GitHub {
        web: format!("http://127.0.0.1:{port}"),
        api: format!("http://127.0.0.1:{port}"),
        client_id: "Iv1.test".into(),
        http: Arc::new(UreqClient::new()),
    };
    let mut code = gh.start("repo").unwrap_or_else(|e| panic!("{e}"));
    code.device_code = "dc-denied".into();
    assert_eq!(gh.poll(&code).ok(), Some(Poll::Denied));
    code.device_code = "dc-old".into();
    assert_eq!(gh.poll(&code).ok(), Some(Poll::Expired));
    let wrong = GitHub {
        client_id: "someone-else".into(),
        ..gh.clone()
    };
    assert!(
        wrong.start("repo").is_err(),
        "a start GitHub refuses is an error"
    );
}
