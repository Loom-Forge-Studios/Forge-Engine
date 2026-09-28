//! Who a remote view signs in as: a token per host, shared by every store the plugin opens.
//!
//! A token never travels through the command bus, a project setting, a URL the store
//! remembers, or a message: GitHub's device flow puts it here and the smart-HTTP client
//! reads it when a request goes to that host. Held in memory for the session (keeping it in
//! the operating system's credential store is a follow-up, ADR 0035).

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

/// One host's credential (HTTP Basic: GitHub takes any user name with a token as the
/// password; `x-access-token` is the documented one).
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub secret: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl Credential {
    /// A bearer token, as GitHub's device flow issues it.
    #[must_use]
    pub fn token(secret: &str) -> Self {
        Self {
            username: "x-access-token".into(),
            secret: secret.to_string(),
        }
    }
}

/// Credentials by host (`github.com`, `git.example.org:3000`). Cheap to clone: shared.
#[derive(Clone, Default)]
pub struct Credentials {
    inner: Arc<RwLock<BTreeMap<String, Credential>>>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.hosts()).finish()
    }
}

/// The `host[:port]` of an `http(s)://` URL.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split('/').next()?;
    let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

impl Credentials {
    /// Remember `c` for `host`.
    pub fn set(&self, host: &str, c: Credential) {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host.to_ascii_lowercase(), c);
    }

    /// Forget `host`'s credential.
    pub fn forget(&self, host: &str) {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&host.to_ascii_lowercase());
    }

    /// The credential for the host of `url`.
    #[must_use]
    pub fn for_url(&self, url: &str) -> Option<Credential> {
        let host = host_of(url)?;
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&host)
            .cloned()
    }

    /// The hosts signed in to.
    #[must_use]
    pub fn hosts(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_go_by_host_and_never_print() {
        let c = Credentials::default();
        c.set("GitHub.com", Credential::token("gho_secret"));
        assert!(c.for_url("https://github.com/ada/orbits.git").is_some());
        assert!(c.for_url("https://gitlab.com/ada/orbits.git").is_none());
        assert_eq!(
            host_of("https://me:pw@git.example.org:3000/a.git").as_deref(),
            Some("git.example.org:3000")
        );
        let shown = format!("{c:?} {:?}", c.for_url("https://github.com/x"));
        assert!(!shown.contains("gho_secret"), "{shown}");
        c.forget("github.com");
        assert!(c.hosts().is_empty());
    }
}
