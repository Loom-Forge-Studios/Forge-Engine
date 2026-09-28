//! Git's smart HTTP protocol (version 0/1), client side: what GitHub, GitLab, Gitea,
//! Forgejo and `git http-backend` serve. gitoxide has no push, so both directions are here,
//! in the few hundred lines the protocol needs:
//!
//! * **Discovery** — `GET <url>/info/refs?service=git-upload-pack|git-receive-pack`: the
//!   refs and the server's capabilities.
//! * **Fetch** — `POST <url>/git-upload-pack` with `want`/`have`/`done`; the reply's pack
//!   (multiplexed on side-band 1, progress on 2, errors on 3) streams straight into
//!   gix-pack, which indexes it into the repository — deltas resolved on every core, never
//!   held whole in memory.
//! * **Push** — `POST <url>/git-receive-pack` with one `<old> <new> <ref>` line per ref
//!   (`atomic` when the server offers it: both refs move or neither) and a pack of the
//!   objects the remote lacks. The old values make it a compare-and-swap: a remote that
//!   moved refuses it, and that is reported as `RemoteMoved`, never forced.
//!
//! Sign-in is HTTP Basic with the host's [`Credentials`] (GitHub: `x-access-token` and the
//! device-flow token); the token is never part of a URL, a message or a log line.

use std::io::{BufReader, Read, Write as _};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use forge_store::StoreError;
use gix::ObjectId;

use super::credentials::Credentials;
use super::http::{HttpClient, HttpRequest};
use super::repo::{HASH, Repo};
use super::transport::{RefUpdate, Refs, Transport};

/// Replies that are not packs (ref advertisements, push reports) are at most this large.
const SMALL: usize = 64 << 20;

/// The smart-HTTP transport of one remote URL.
pub(crate) struct SmartHttp {
    /// The URL requests go to (credentials stripped).
    base: String,
    /// For messages.
    name: String,
    creds: Credentials,
    /// `user:password@` given in the URL itself, used when no stored credential matches.
    inline: Option<(String, String)>,
    http: Arc<dyn HttpClient>,
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = match c.len() {
            1 => u32::from(c[0]) << 16,
            2 => (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8),
            _ => (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]),
        };
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

/// Encode one pkt-line.
pub(crate) fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    let _ = write!(out, "{:04x}", data.len() + 4);
    out.extend_from_slice(data);
}

/// The flush packet.
pub(crate) const FLUSH: &[u8] = b"0000";

/// One decoded pkt-line: `None` is a flush (or delimiter).
pub(crate) fn read_pkt(r: &mut dyn Read) -> Result<Option<Vec<u8>>, String> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .map_err(|e| format!("the reply ended early: {e}"))?;
    let text = std::str::from_utf8(&len).map_err(|_| "a malformed pkt-line".to_string())?;
    let n =
        usize::from_str_radix(text, 16).map_err(|_| format!("a malformed pkt-line {text:?}"))?;
    if n < 4 {
        return Ok(None);
    }
    let mut data = vec![0u8; n - 4];
    r.read_exact(&mut data)
        .map_err(|e| format!("the reply ended early: {e}"))?;
    Ok(Some(data))
}

fn line(data: &[u8]) -> &str {
    std::str::from_utf8(data)
        .unwrap_or("")
        .trim_end_matches('\n')
}

/// Parse a ref advertisement (after its `# service=` header).
pub(crate) fn parse_advertisement(body: &[u8]) -> Result<(Refs, Vec<String>), String> {
    let mut r = body;
    let first = read_pkt(&mut r)?.ok_or("an empty advertisement")?;
    if line(&first).starts_with("# service=") {
        // The smart-HTTP header, then a flush.
        if read_pkt(&mut r)?.is_some() {
            return Err("the advertisement header is not followed by a flush".into());
        }
    } else {
        r = body;
    }
    let mut refs = Refs::new();
    let mut caps = Vec::new();
    let mut first_ref = true;
    while let Some(data) = read_pkt(&mut r)? {
        let (text, cap) = match data.iter().position(|b| *b == 0) {
            Some(i) => (&data[..i], Some(&data[i + 1..])),
            None => (&data[..], None),
        };
        if first_ref {
            if let Some(c) = cap {
                caps = line(c)
                    .split(' ')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            first_ref = false;
        }
        let l = line(text);
        let Some((oid, name)) = l.split_once(' ') else {
            return Err(format!("a malformed ref line {l:?}"));
        };
        if name == "capabilities^{}" {
            continue; // an empty repository
        }
        let oid = ObjectId::from_hex(oid.as_bytes()).map_err(|e| format!("{l:?}: {e}"))?;
        refs.insert(name.to_string(), oid);
    }
    Ok((refs, caps))
}

/// A reader over side-band-multiplexed pkt-lines: band 1 is the data, band 2 progress
/// (dropped), band 3 an error. `ACK`/`NAK` lines before the first band are skipped.
struct SideBand<R: Read> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> Read for SideBand<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            if self.done {
                return Ok(0);
            }
            let pkt = read_pkt(&mut self.inner).map_err(std::io::Error::other)?;
            let Some(data) = pkt else {
                self.done = true;
                return Ok(0);
            };
            match data.first() {
                Some(1) => {
                    self.buf = data;
                    self.pos = 1;
                }
                Some(2) => {}
                Some(3) => {
                    return Err(std::io::Error::other(format!(
                        "the remote said: {}",
                        String::from_utf8_lossy(&data[1..]).trim()
                    )));
                }
                _ if data.starts_with(b"ACK") || data.starts_with(b"NAK") => {}
                _ => {
                    return Err(std::io::Error::other(format!(
                        "an unexpected reply line {:?}",
                        String::from_utf8_lossy(&data).trim()
                    )));
                }
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A pack of `objects` (whole objects, zlib-compressed; no deltas).
pub(crate) fn write_pack(repo: &Repo, objects: &[ObjectId]) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    let n = u32::try_from(objects.len()).map_err(|_| StoreError::Remote {
        remote: repo.git_dir().display().to_string(),
        why: "more than 4 billion objects in one push".into(),
    })?;
    out.extend_from_slice(&n.to_be_bytes());
    for id in objects {
        let (kind, data) = repo.read(id)?;
        let ty: u8 = match kind {
            gix::objs::Kind::Commit => 1,
            gix::objs::Kind::Tree => 2,
            gix::objs::Kind::Blob => 3,
            gix::objs::Kind::Tag => 4,
        };
        let mut size = data.len();
        let mut b = (ty << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            out.push(b | 0x80);
            b = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(b);
        let mut z = flate2::write::ZlibEncoder::new(&mut out, flate2::Compression::default());
        z.write_all(&data)
            .and_then(|()| z.finish().map(drop))
            .map_err(|e| StoreError::Remote {
                remote: repo.git_dir().display().to_string(),
                why: format!("compressing a pack: {e}"),
            })?;
    }
    let mut h = gix::hash::hasher(HASH);
    h.update(&out);
    let sum = h.try_finalize().map_err(|e| StoreError::Remote {
        remote: repo.git_dir().display().to_string(),
        why: e.to_string(),
    })?;
    out.extend_from_slice(sum.as_bytes());
    Ok(out)
}

/// Index a pack stream into `repo`.
pub(crate) fn receive_pack(repo: &Repo, pack: &mut dyn std::io::BufRead) -> Result<(), String> {
    let dir = repo.pack_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let interrupt = AtomicBool::new(false);
    let out = gix_pack::Bundle::write_to_directory(
        pack,
        Some(&dir),
        &mut gix::progress::Discard,
        &interrupt,
        None::<gix::objs::find::Never>,
        HASH,
        gix_pack::bundle::write::Options::default(),
    )
    .map_err(|e| format!("the received pack does not index: {e}"))?;
    // The refs are set right after this; nothing collects packs meanwhile.
    if let Some(keep) = out.keep_path {
        let _ = std::fs::remove_file(keep);
    }
    Ok(())
}

impl SmartHttp {
    pub(crate) fn new(url: &str, creds: Credentials, http: Arc<dyn HttpClient>) -> Self {
        let url = url.trim().trim_end_matches('/');
        let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
        let (auth, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
        let (inline, host) = match auth.rsplit_once('@') {
            Some((cred, host)) => {
                let (u, p) = cred.split_once(':').unwrap_or((cred, ""));
                (Some((u.to_string(), p.to_string())), host)
            }
            None => (None, auth),
        };
        let base = if path.is_empty() {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}/{path}")
        };
        Self {
            name: base.clone(),
            base,
            creds,
            inline,
            http,
        }
    }

    fn request(&self, mut req: HttpRequest) -> Result<super::http::HttpResponse, StoreError> {
        let auth = match self.creds.for_url(&self.base) {
            Some(c) => Some((c.username, c.secret)),
            None => self.inline.clone(),
        };
        if let Some((u, p)) = auth {
            req = req.header(
                "Authorization",
                &format!("Basic {}", base64(format!("{u}:{p}").as_bytes())),
            );
        }
        let res = self.http.send(req).map_err(|why| StoreError::Remote {
            remote: self.name.clone(),
            why,
        })?;
        match res.status {
            200 => Ok(res),
            401 | 403 => Err(StoreError::Unauthorized {
                remote: self.name.clone(),
                why: format!("HTTP {}", res.status),
            }),
            404 => Err(StoreError::Remote {
                remote: self.name.clone(),
                why: "not found (check the URL; a private repository may also answer this until you sign in)".into(),
            }),
            s => Err(StoreError::Remote {
                remote: self.name.clone(),
                why: format!("HTTP {s}"),
            }),
        }
    }

    fn remote(&self, why: impl Into<String>) -> StoreError {
        StoreError::Remote {
            remote: self.name.clone(),
            why: why.into(),
        }
    }

    fn discover(&self, service: &str) -> Result<(Refs, Vec<String>), StoreError> {
        let res = self.request(
            HttpRequest::get(&format!("{}/info/refs?service={service}", self.base))
                .header("Accept", "*/*"),
        )?;
        let smart = format!("application/x-{service}-advertisement");
        if !res.content_type.starts_with(&smart) {
            return Err(self.remote(format!(
                "not a smart-HTTP Git server (it answered {:?}; the dumb protocol is not supported)",
                res.content_type
            )));
        }
        let body = res.bytes(SMALL).map_err(|e| self.remote(e))?;
        parse_advertisement(&body).map_err(|e| self.remote(e))
    }
}

impl Transport for SmartHttp {
    fn name(&self) -> &str {
        &self.name
    }

    fn refs(&mut self) -> Result<Refs, StoreError> {
        Ok(self.discover("git-upload-pack")?.0)
    }

    fn fetch(
        &mut self,
        into: &Repo,
        wants: &[ObjectId],
        haves: &[ObjectId],
    ) -> Result<(), StoreError> {
        if wants.is_empty() {
            return Ok(());
        }
        let (_, caps) = self.discover("git-upload-pack")?;
        let has = |c: &str| caps.iter().any(|x| x == c);
        let band = if has("side-band-64k") {
            "side-band-64k"
        } else if has("side-band") {
            "side-band"
        } else {
            return Err(self.remote("the server offers no side-band (every Git host does)"));
        };
        let mut want_caps = vec![band.to_string()];
        for c in ["ofs-delta", "no-progress"] {
            if has(c) {
                want_caps.push(c.to_string());
            }
        }
        want_caps.push(concat!("agent=forge/", env!("CARGO_PKG_VERSION")).to_string());
        let mut body = Vec::new();
        for (i, w) in wants.iter().enumerate() {
            if i == 0 {
                pkt(
                    &mut body,
                    format!("want {w} {}\n", want_caps.join(" ")).as_bytes(),
                );
            } else {
                pkt(&mut body, format!("want {w}\n").as_bytes());
            }
        }
        body.extend_from_slice(FLUSH);
        for h in haves {
            pkt(&mut body, format!("have {h}\n").as_bytes());
        }
        pkt(&mut body, b"done\n");
        let res = self.request(
            HttpRequest::post(&format!("{}/git-upload-pack", self.base), body)
                .header("Content-Type", "application/x-git-upload-pack-request")
                .header("Accept", "application/x-git-upload-pack-result"),
        )?;
        let demux = SideBand {
            inner: res.body,
            buf: Vec::new(),
            pos: 0,
            done: false,
        };
        let mut pack = BufReader::with_capacity(1 << 16, demux);
        receive_pack(into, &mut pack).map_err(|e| self.remote(e))
    }

    fn push(
        &mut self,
        from: &Repo,
        updates: &[RefUpdate],
        objects: &[ObjectId],
    ) -> Result<(), StoreError> {
        if updates.is_empty() {
            return Ok(());
        }
        let (refs, caps) = self.discover("git-receive-pack")?;
        for u in updates {
            if refs.get(u.name).copied() != u.old {
                return Err(StoreError::RemoteMoved {
                    remote: self.name.clone(),
                });
            }
        }
        let has = |c: &str| caps.iter().any(|x| x == c);
        if !has("report-status") {
            return Err(self.remote("the server offers no report-status"));
        }
        let mut want = vec!["report-status".to_string()];
        if has("atomic") {
            want.push("atomic".into());
        }
        want.push(concat!("agent=forge/", env!("CARGO_PKG_VERSION")).to_string());
        let zero = ObjectId::null(HASH);
        let mut body = Vec::new();
        for (i, u) in updates.iter().enumerate() {
            let old = u.old.unwrap_or(zero);
            let mut l = format!("{old} {} {}", u.new, u.name).into_bytes();
            if i == 0 {
                l.push(0);
                l.extend_from_slice(want.join(" ").as_bytes());
            }
            l.push(b'\n');
            pkt(&mut body, &l);
        }
        body.extend_from_slice(FLUSH);
        body.extend_from_slice(&write_pack(from, objects)?);
        let res = self.request(
            HttpRequest::post(&format!("{}/git-receive-pack", self.base), body)
                .header("Content-Type", "application/x-git-receive-pack-request")
                .header("Accept", "application/x-git-receive-pack-result"),
        )?;
        let reply = res.bytes(SMALL).map_err(|e| self.remote(e))?;
        let mut r = &reply[..];
        let mut unpacked = false;
        let mut moved = false;
        let mut failures = Vec::new();
        let mut accepted = 0usize;
        while let Some(data) = read_pkt(&mut r).map_err(|e| self.remote(e))? {
            let l = line(&data);
            if let Some(rest) = l.strip_prefix("unpack ") {
                if rest != "ok" {
                    return Err(
                        self.remote(format!("the remote could not unpack the push: {rest}"))
                    );
                }
                unpacked = true;
            } else if l.starts_with("ok ") {
                accepted += 1;
            } else if let Some(rest) = l.strip_prefix("ng ") {
                let why = rest.split_once(' ').map_or("", |(_, w)| w);
                if ["fetch first", "non-fast-forward", "stale", "lock", "atomic"]
                    .iter()
                    .any(|k| why.contains(k))
                {
                    moved = true;
                }
                failures.push(rest.to_string());
            }
        }
        if moved
            && failures.iter().all(|f| {
                ["fetch first", "non-fast-forward", "stale", "lock", "atomic"]
                    .iter()
                    .any(|k| f.contains(k))
            })
        {
            return Err(StoreError::RemoteMoved {
                remote: self.name.clone(),
            });
        }
        if !failures.is_empty() {
            return Err(self.remote(format!("the remote refused: {}", failures.join("; "))));
        }
        if !unpacked || accepted != updates.len() {
            return Err(self.remote("the remote's report is incomplete"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn an_advertisement_parses_with_its_capabilities() {
        let mut body = Vec::new();
        pkt(&mut body, b"# service=git-upload-pack\n");
        body.extend_from_slice(FLUSH);
        let a = "1".repeat(40);
        let b = "2".repeat(40);
        pkt(
            &mut body,
            format!("{a} refs/heads/main\0side-band-64k ofs-delta agent=git/2\n").as_bytes(),
        );
        pkt(&mut body, format!("{b} refs/forge/blobs\n").as_bytes());
        body.extend_from_slice(FLUSH);
        let (refs, caps) = parse_advertisement(&body).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs["refs/heads/main"].to_string(), a);
        assert!(caps.iter().any(|c| c == "side-band-64k"));
        // An empty repository advertises only its capabilities.
        let mut empty = Vec::new();
        pkt(
            &mut empty,
            format!("{} capabilities^{{}}\0report-status\n", "0".repeat(40)).as_bytes(),
        );
        empty.extend_from_slice(FLUSH);
        let (refs, caps) = parse_advertisement(&empty).unwrap_or_else(|e| panic!("{e}"));
        assert!(refs.is_empty());
        assert_eq!(caps, ["report-status"]);
    }

    #[test]
    fn urls_carry_no_credentials_into_messages() {
        let s = SmartHttp::new(
            "https://ada:hunter2@git.example.org/ada/orbits.git/",
            Credentials::default(),
            Arc::new(super::super::http::UreqClient::new()),
        );
        assert_eq!(s.name(), "https://git.example.org/ada/orbits.git");
        assert!(s.inline.is_some());
    }
}
