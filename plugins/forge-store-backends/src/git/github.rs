//! GitHub sign-in with the OAuth **device flow** and "repo created for you" (Ch.33.5: the
//! new-project dialog's *GitHub* option).
//!
//! The device flow needs no browser redirect and no secret in the editor: the editor asks
//! GitHub for a code ([`GitHub::start`]), shows the user the code and the address to enter
//! it at, and polls ([`GitHub::poll`]) — at the interval GitHub names, slowing down when told
//! to — until the user approves (a token), declines, or the code expires. Each poll is one
//! request that returns at once, so the caller decides when to poll (the editor core, on its
//! own schedule); nothing here blocks or sleeps.
//!
//! Endpoints are fields ([`GitHub::web`], [`GitHub::api`]), so the whole flow is tested
//! against a mock server on loopback through the production HTTP client. The OAuth app's
//! client id is configuration: Forge's own app is not registered yet (gate row
//! `C-github-oauth-app`, UNBUILT: the owner registers it), so `FORGE_GITHUB_CLIENT_ID`
//! names one.

use std::sync::Arc;

use forge_store::StoreError;
use serde::Deserialize;

use super::http::{HttpClient, HttpRequest};

/// The environment variable naming the OAuth app's client id.
pub const CLIENT_ID_ENV: &str = "FORGE_GITHUB_CLIENT_ID";

/// What GitHub hands back to start a sign-in: show `user_code` and `verification_uri`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    /// Polled with; never shown.
    pub device_code: String,
    /// The code the user types at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the code expires.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// What one poll found.
#[derive(Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not answered yet: poll again after the interval.
    Pending,
    /// Polling too fast: poll again after this many seconds (the new interval).
    SlowDown(u64),
    /// Approved: the token (put it in the credentials; never show or log it).
    Token(String),
    /// The user declined.
    Denied,
    /// The code expired: start again.
    Expired,
}

impl std::fmt::Debug for Poll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Poll::Pending => f.write_str("Pending"),
            Poll::SlowDown(s) => write!(f, "SlowDown({s})"),
            Poll::Token(_) => f.write_str("Token(<redacted>)"),
            Poll::Denied => f.write_str("Denied"),
            Poll::Expired => f.write_str("Expired"),
        }
    }
}

/// A repository GitHub created.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CreatedRepo {
    /// `owner/name`.
    pub full_name: String,
    /// What to link as the project's remote.
    pub clone_url: String,
    pub html_url: String,
}

/// A GitHub (or GitHub Enterprise) instance and the OAuth app to sign in with.
#[derive(Clone)]
pub struct GitHub {
    /// `https://github.com` — the device-code and token endpoints.
    pub web: String,
    /// `https://api.github.com` — the REST API.
    pub api: String,
    pub client_id: String,
    pub http: Arc<dyn HttpClient>,
}

impl std::fmt::Debug for GitHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHub")
            .field("web", &self.web)
            .field("api", &self.api)
            .finish_non_exhaustive()
    }
}

fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    let enc = |s: &str| {
        let mut o = String::new();
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                o.push(char::from(b));
            } else {
                o.push_str(&format!("%{b:02X}"));
            }
        }
        o
    };
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

impl GitHub {
    /// github.com with the client id from [`CLIENT_ID_ENV`] (`None`: no app is configured).
    #[must_use]
    pub fn from_env(http: Arc<dyn HttpClient>) -> Option<Self> {
        let id = std::env::var(CLIENT_ID_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())?;
        Some(Self {
            web: "https://github.com".into(),
            api: "https://api.github.com".into(),
            client_id: id.trim().to_string(),
            http,
        })
    }

    fn err(&self, why: impl Into<String>) -> StoreError {
        StoreError::Remote {
            remote: self.web.clone(),
            why: why.into(),
        }
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, req: HttpRequest) -> Result<(u16, T), StoreError> {
        let res = self.http.send(req).map_err(|e| self.err(e))?;
        let status = res.status;
        let body = res.bytes(1 << 20).map_err(|e| self.err(e))?;
        if status == 401 || status == 403 {
            return Err(StoreError::Unauthorized {
                remote: self.web.clone(),
                why: format!("HTTP {status}"),
            });
        }
        let v = serde_json::from_slice(&body).map_err(|e| {
            self.err(format!(
                "HTTP {status}: the reply is not what GitHub sends: {e}"
            ))
        })?;
        Ok((status, v))
    }

    /// Ask for a device code (`scope`: `repo` to push to and create repositories).
    pub fn start(&self, scope: &str) -> Result<DeviceCode, StoreError> {
        let (status, code): (u16, DeviceCode) = self.json(
            HttpRequest::post(
                &format!("{}/login/device/code", self.web),
                form(&[("client_id", &self.client_id), ("scope", scope)]),
            )
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded"),
        )?;
        if status != 200 {
            return Err(self.err(format!("HTTP {status} starting the sign-in")));
        }
        Ok(code)
    }

    /// Poll once (see the module docs).
    pub fn poll(&self, code: &DeviceCode) -> Result<Poll, StoreError> {
        let (_, r): (u16, TokenReply) = self.json(
            HttpRequest::post(
                &format!("{}/login/oauth/access_token", self.web),
                form(&[
                    ("client_id", &self.client_id),
                    ("device_code", &code.device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ]),
            )
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded"),
        )?;
        if let Some(t) = r.access_token.filter(|t| !t.is_empty()) {
            return Ok(Poll::Token(t));
        }
        match r.error.as_deref() {
            Some("authorization_pending") => Ok(Poll::Pending),
            Some("slow_down") => Ok(Poll::SlowDown(
                r.interval.unwrap_or(code.interval.saturating_add(5)),
            )),
            Some("expired_token") => Ok(Poll::Expired),
            Some("access_denied") => Ok(Poll::Denied),
            Some(e) => Err(self.err(format!(
                "sign-in failed: {e}{}",
                r.error_description
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ))),
            None => Err(self.err("the token reply holds neither a token nor an error")),
        }
    }

    /// The signed-in user's login.
    pub fn user(&self, token: &str) -> Result<String, StoreError> {
        let (status, u): (u16, User) = self.json(
            HttpRequest::get(&format!("{}/user", self.api))
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", &format!("Bearer {token}")),
        )?;
        if status != 200 {
            return Err(self.err(format!("HTTP {status} reading the user")));
        }
        Ok(u.login)
    }

    /// Create a private repository named `name` for the signed-in user (Ch.33.5: "repo
    /// created for you"). A name already taken is an error naming it.
    pub fn create_repo(&self, token: &str, name: &str) -> Result<CreatedRepo, StoreError> {
        let body = serde_json::json!({
            "name": name,
            "private": true,
            "description": "A Forge project",
            "auto_init": false,
        });
        let req = HttpRequest::post(
            &format!("{}/user/repos", self.api),
            body.to_string().into_bytes(),
        )
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"));
        let res = self.http.send(req).map_err(|e| self.err(e))?;
        let status = res.status;
        let bytes = res.bytes(1 << 20).map_err(|e| self.err(e))?;
        match status {
            200 | 201 => serde_json::from_slice(&bytes)
                .map_err(|e| self.err(format!("the created repository does not read: {e}"))),
            401 | 403 => Err(StoreError::Unauthorized {
                remote: self.api.clone(),
                why: format!("HTTP {status}: the token cannot create repositories (scope `repo`)"),
            }),
            422 => Err(self.err(format!(
                "a repository named {name:?} already exists: pick another name, or link it as the remote"
            ))),
            s => Err(self.err(format!("HTTP {s} creating the repository"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_percent_encoded_and_tokens_never_print() {
        assert_eq!(
            String::from_utf8(form(&[("a b", "x&y=z"), ("g", "urn:ietf")])).unwrap_or_default(),
            "a%20b=x%26y%3Dz&g=urn%3Aietf"
        );
        assert_eq!(
            format!("{:?}", Poll::Token("gho_x".into())),
            "Token(<redacted>)"
        );
    }
}
