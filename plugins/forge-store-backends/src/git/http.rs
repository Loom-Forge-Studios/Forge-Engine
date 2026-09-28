//! The HTTP client the smart-HTTP transport and GitHub's device flow speak through.
//!
//! [`HttpClient`] is a trait so the protocol code is tested against real servers on
//! loopback (a mock of GitHub's OAuth endpoints; real `git` behind a small smart-HTTP
//! front) through the very client production uses: [`UreqClient`] — rustls, the operating
//! system's trust store, no bundled root list, HTTP status never turned into an error (the
//! protocol code reads 401, 404, 422 itself).

use std::io::Read;
use std::time::Duration;

/// A request.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// `GET` or `POST`.
    pub post: bool,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Sent with a `POST`.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// A `GET`.
    #[must_use]
    pub fn get(url: &str) -> Self {
        Self {
            post: false,
            url: url.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A `POST` of `body`.
    #[must_use]
    pub fn post(url: &str, body: Vec<u8>) -> Self {
        Self {
            post: true,
            url: url.to_string(),
            headers: Vec::new(),
            body,
        }
    }

    /// With a header.
    #[must_use]
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
}

/// A response: its status, content type and a streamed body (a fetched pack can be large;
/// it is never held whole in memory by the client).
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Box<dyn Read + Send>,
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

impl HttpResponse {
    /// The whole body (for small replies: ref advertisements, JSON).
    pub fn bytes(mut self, limit: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        (&mut self.body)
            .take(limit as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|e| e.to_string())?;
        if out.len() > limit {
            return Err(format!("the reply is larger than {limit} bytes"));
        }
        Ok(out)
    }
}

/// Sends HTTP requests. The error is a transport failure (no connection, TLS); an HTTP
/// status is a response.
pub trait HttpClient: Send + Sync {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, String>;
}

/// The production client (see the module docs).
pub struct UreqClient {
    agent: ureq::Agent,
}

impl std::fmt::Debug for UreqClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UreqClient")
    }
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqClient {
    /// A client: 30 s to connect, 60 s for a reply to start, no overall limit (a large
    /// fetch takes as long as it takes), the OS trust store.
    #[must_use]
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // Some Git hosts only speak the smart protocol to a `git/` user agent.
            .user_agent(concat!("git/2.0 (forge/", env!("CARGO_PKG_VERSION"), ")"))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .tls_config(tls)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl HttpClient for UreqClient {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, String> {
        let res = if req.post {
            let mut r = self.agent.post(&req.url);
            for (k, v) in &req.headers {
                r = r.header(k, v);
            }
            r.send(&req.body[..])
        } else {
            let mut r = self.agent.get(&req.url);
            for (k, v) in &req.headers {
                r = r.header(k, v);
            }
            r.call()
        }
        .map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok(HttpResponse {
            status,
            content_type,
            body: Box::new(res.into_body().into_reader()),
        })
    }
}
