//! A tiny explicit little-endian codec for binary artefacts. Every multi-byte value is
//! written LE and every length is a `u32`/`u64` prefix, so an artefact's bytes — and so its
//! content address — are identical on every platform.

use bytes::Bytes;

/// Appends LE values to a buffer.
#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn with_magic(magic: &[u8; 4], version: u32) -> Self {
        let mut w = Self::default();
        w.buf.extend_from_slice(magic);
        w.u32(version);
        w
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u128(&mut self, v: u128) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    pub fn len(&mut self, n: usize) {
        self.u64(n as u64);
    }
    pub fn str(&mut self, s: &str) {
        self.len(s.len());
        self.buf.extend_from_slice(s.as_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.len(b.len());
        self.buf.extend_from_slice(b);
    }
    pub fn finish(self) -> Bytes {
        Bytes::from(self.buf)
    }
}

/// Reads LE values; every read is bounds-checked (artefacts can come from anywhere).
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
    /// The backing buffer, so `bytes()` can hand out zero-copy slices.
    owner: Option<&'a Bytes>,
}

impl<'a> Reader<'a> {
    pub fn new(owner: &'a Bytes, magic: &[u8; 4], version: u32) -> Result<Self, String> {
        let mut r = Self {
            buf: owner,
            at: 0,
            owner: Some(owner),
        };
        let m = r.take(4)?;
        if m != magic {
            return Err(format!(
                "bad magic {:?}, expected {:?}",
                String::from_utf8_lossy(m),
                String::from_utf8_lossy(magic)
            ));
        }
        let v = r.u32()?;
        if v != version {
            return Err(format!("format version {v}, expected {version}"));
        }
        Ok(r)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| format!("truncated at byte {} (wanted {n} more)", self.at))?;
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    pub fn u128(&mut self) -> Result<u128, String> {
        Ok(u128::from_le_bytes(self.arr()?))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }
    pub fn len(&mut self) -> Result<usize, String> {
        let n = self.u64()?;
        let n = usize::try_from(n).map_err(|_| format!("length {n} overflows"))?;
        if n > self.buf.len() - self.at {
            return Err(format!("length {n} runs past the end"));
        }
        Ok(n)
    }
    pub fn str(&mut self) -> Result<String, String> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|e| e.to_string())
    }
    pub fn bytes(&mut self) -> Result<Bytes, String> {
        let n = self.len()?;
        let start = self.at;
        self.take(n)?;
        Ok(match self.owner {
            Some(o) => o.slice(start..start + n),
            None => Bytes::copy_from_slice(&self.buf[start..start + n]),
        })
    }
    pub fn end(&self) -> Result<(), String> {
        if self.at == self.buf.len() {
            Ok(())
        } else {
            Err(format!("{} trailing bytes", self.buf.len() - self.at))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_truncation() {
        let mut w = Writer::with_magic(b"TEST", 3);
        w.u8(7);
        w.u32(0xdead_beef);
        w.f64(-0.0);
        w.str("héllo");
        w.bytes(&[1, 2, 3]);
        w.u128(u128::MAX - 5);
        let b = w.finish();
        let mut r = Reader::new(&b, b"TEST", 3).expect("header");
        assert_eq!(r.u8(), Ok(7));
        assert_eq!(r.u32(), Ok(0xdead_beef));
        assert_eq!(r.f64().map(f64::to_bits), Ok((-0.0f64).to_bits()));
        assert_eq!(r.str().as_deref(), Ok("héllo"));
        assert_eq!(r.bytes().as_deref(), Ok(&[1u8, 2, 3][..]));
        assert_eq!(r.u128(), Ok(u128::MAX - 5));
        assert!(r.end().is_ok());
        let cut = b.slice(..b.len() - 3);
        let mut r = Reader::new(&cut, b"TEST", 3).expect("header");
        assert!(r.u8().is_ok() && r.u32().is_ok() && r.f64().is_ok());
        assert!(r.str().is_ok() && r.bytes().is_ok());
        assert!(r.u128().is_err());
        assert!(Reader::new(&b, b"NOPE", 3).is_err());
        assert!(Reader::new(&b, b"TEST", 4).is_err());
    }
}
