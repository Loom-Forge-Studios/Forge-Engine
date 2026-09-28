//! Author a plugin in WebAssembly text: [`component`] wraps a core module body in the
//! component boilerplate of the `forge:plugin` world (the canonical-ABI lifts and lowers, a
//! bump allocator reset after every call, the host imports asked for), so a small plugin —
//! and every WASM fixture in Forge's own tests — is one readable core module.
//!
//! The body is the inside of a core `(module ...)`. It must define
//!
//! ```wat
//! (func (export "call") (param $point i32) (param $point_len i32)
//!                       (param $key i32) (param $key_len i32)
//!                       (param $in i32) (param $in_len i32) (result i32) ...)
//! ```
//!
//! and may use what the scaffold defines:
//!
//! * `$ok (param ptr len) (result i32)` / `$err (param ptr len) (result i32)` — build the
//!   `result<list<u8>, string>` return value (return what they return);
//! * `$alloc (param size) (result ptr)` — memory that lives until the call returns;
//! * `$log (param ptr len)` when `host` is imported; `$read (param path len ret)` when
//!   `project-read` is (it writes a `result<list<u8>, string>` at `ret`: tag byte at `ret`,
//!   pointer at `ret+4`, length at `ret+8` — the same layout `call` returns, so a guest can
//!   return `ret` as is).
//!
//! Static data belongs below byte 32768 (the allocator starts there); bytes 16..28 hold the
//! return value.

use std::fmt::Write as _;

use crate::host::{HOST_INTERFACE, PROJECT_READ_INTERFACE};

/// Where the allocator starts; static data goes below.
pub const HEAP_BASE: u32 = 32768;

/// A `(data ...)` segment placing `bytes` at `offset` (every byte escaped, so any content —
/// JSON, RON, binary — is safe).
#[must_use]
pub fn data(offset: u32, bytes: &[u8]) -> String {
    let mut s = format!("(data (i32.const {offset}) \"");
    for b in bytes {
        let _ = write!(s, "\\{b:02x}");
    }
    s.push_str("\")");
    s
}

/// A whole plugin component: `imports` names the host interfaces to import (`host`,
/// `project-read`), `body` is the core module body (see the module docs).
#[must_use]
pub fn component(imports: &[&str], body: &str) -> String {
    let log = imports.contains(&"host");
    let read = imports.contains(&"project-read");
    let mut c = String::from("(component\n");
    if log {
        let _ = writeln!(
            c,
            "  (import \"{HOST_INTERFACE}\" (instance $host (export \"log\" (func (param \"msg\" string)))))"
        );
    }
    if read {
        let _ = writeln!(
            c,
            "  (import \"{PROJECT_READ_INTERFACE}\" (instance $project (export \"read\" (func (param \"path\" string) (result (result (list u8) (error string)))))))"
        );
    }
    let _ = write!(
        c,
        r#"  (core module $libc
    (memory (export "memory") 1)
    (global $bump (mut i32) (i32.const {HEAP_BASE}))
    (func (export "realloc") (param $old i32) (param $osz i32) (param $align i32) (param $nsz i32) (result i32)
      (local $p i32) (local $have i32)
      (local.set $p (i32.and (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
                             (i32.sub (i32.const 0) (local.get $align))))
      (global.set $bump (i32.add (local.get $p) (local.get $nsz)))
      (local.set $have (i32.mul (memory.size) (i32.const 65536)))
      (if (i32.gt_u (global.get $bump) (local.get $have))
        (then (if (i32.eq (memory.grow (i32.add (i32.shr_u (i32.sub (global.get $bump) (local.get $have)) (i32.const 16)) (i32.const 1)))
                          (i32.const -1))
                (then unreachable))))
      (if (local.get $old) (then (memory.copy (local.get $p) (local.get $old) (local.get $osz))))
      (local.get $p))
    (func (export "reset") (global.set $bump (i32.const {HEAP_BASE}))))
  (core instance $libc (instantiate $libc))
"#
    );
    if log {
        c.push_str("  (core func $log (canon lower (func $host \"log\") (memory (core memory $libc \"memory\"))))\n");
    }
    if read {
        c.push_str("  (core func $read (canon lower (func $project \"read\") (memory (core memory $libc \"memory\")) (realloc (core func $libc \"realloc\"))))\n");
    }
    c.push_str(
        r#"  (core module $main
    (import "libc" "memory" (memory 1))
    (import "libc" "realloc" (func $realloc (param i32 i32 i32 i32) (result i32)))
    (import "libc" "reset" (func $reset))
"#,
    );
    if log {
        c.push_str("    (import \"host\" \"log\" (func $log (param i32 i32)))\n");
    }
    if read {
        c.push_str("    (import \"project\" \"read\" (func $read (param i32 i32 i32)))\n");
    }
    c.push_str(
        r#"    (func $alloc (param $n i32) (result i32)
      (call $realloc (i32.const 0) (i32.const 0) (i32.const 8) (local.get $n)))
    (func $ok (param $p i32) (param $n i32) (result i32)
      (i32.store8 (i32.const 16) (i32.const 0))
      (i32.store (i32.const 20) (local.get $p))
      (i32.store (i32.const 24) (local.get $n))
      (i32.const 16))
    (func $err (param $p i32) (param $n i32) (result i32)
      (i32.store8 (i32.const 16) (i32.const 1))
      (i32.store (i32.const 20) (local.get $p))
      (i32.store (i32.const 24) (local.get $n))
      (i32.const 16))
    (func (export "post") (param i32) (call $reset))
"#,
    );
    c.push_str(body);
    c.push_str("\n  )\n  (core instance $main (instantiate $main (with \"libc\" (instance $libc))");
    if log {
        c.push_str(" (with \"host\" (instance (export \"log\" (func $log))))");
    }
    if read {
        c.push_str(" (with \"project\" (instance (export \"read\" (func $read))))");
    }
    c.push_str(
        r#"))
  (func (export "call") (param "point" string) (param "key" string) (param "input" (list u8))
        (result (result (list u8) (error string)))
    (canon lift (core func $main "call") (memory (core memory $libc "memory"))
      (realloc (core func $libc "realloc")) (post-return (core func $main "post"))))
)
"#,
    );
    c
}

/// A plugin whose `call` answers every item with `reply` (a fixture shape used widely:
/// a command's plan, a preset's RON).
#[must_use]
pub fn constant(reply: &[u8]) -> String {
    let len = reply.len();
    component(
        &[],
        &format!(
            "{}\n    (func (export \"call\") (param i32 i32 i32 i32 i32 i32) (result i32)\n      (call $ok (i32.const 1024) (i32.const {len})))",
            data(1024, reply)
        ),
    )
}

/// A plugin whose `call` answers each point with its own constant reply (`(point name,
/// reply)`, e.g. `("Command", plan)`, `("EditorPanel", view)`), and an error for any other
/// point: a fixture for a plugin serving items of several points.
#[must_use]
pub fn by_point(replies: &[(&str, &[u8])]) -> String {
    let mut data_segs = String::new();
    let mut arms = String::new();
    let mut at: u32 = 1024;
    for (point, reply) in replies {
        let (p_at, p_len) = (at, point.len());
        data_segs.push_str(&data(p_at, point.as_bytes()));
        data_segs.push('\n');
        at += u32::try_from(p_len).unwrap_or(0) + 8;
        let (r_at, r_len) = (at, reply.len());
        data_segs.push_str(&data(r_at, reply));
        data_segs.push('\n');
        at += u32::try_from(r_len).unwrap_or(0) + 8;
        let _ = writeln!(
            arms,
            "      (if (call $eq (local.get $p) (local.get $pn) (i32.const {p_at}) (i32.const {p_len}))\n        (then (return (call $ok (i32.const {r_at}) (i32.const {r_len})))))"
        );
    }
    let err = b"no item of this point";
    let e_at = at;
    data_segs.push_str(&data(e_at, err));
    component(
        &[],
        &format!(
            r#"{data_segs}
    (func $eq (param $a i32) (param $an i32) (param $b i32) (param $bn i32) (result i32)
      (local $i i32)
      (if (i32.ne (local.get $an) (local.get $bn)) (then (return (i32.const 0))))
      (block $done
        (loop $next
          (br_if $done (i32.ge_u (local.get $i) (local.get $an)))
          (if (i32.ne (i32.load8_u (i32.add (local.get $a) (local.get $i)))
                      (i32.load8_u (i32.add (local.get $b) (local.get $i))))
            (then (return (i32.const 0))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next)))
      (i32.const 1))
    (func (export "call") (param $p i32) (param $pn i32) (param i32 i32 i32 i32) (result i32)
{arms}      (call $err (i32.const {e_at}) (i32.const {})))"#,
            err.len()
        ),
    )
}
