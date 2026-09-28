//! **A quiet machine** before a timed body starts (WP-40, ADR 0057).
//!
//! The machine-wide timed lock ([`super::machine_lock`]) keeps timed bodies apart from each
//! other and from GPU tests, and the High priority class puts a timed body ahead of normal
//! background work — but neither sees *another worktree's* `rustc` and `link.exe`. A dozen
//! compiler processes on twelve cores still take cache, memory bandwidth, turbo headroom and
//! the scheduler's attention from a timed body, whatever its priority: WP-36's integration saw
//! the streaming walk's frames and the perf gate's rows fail in two full runs while the other
//! lane compiled (both pass alone, 30 of 30), and WP-U17/U18/39 saw the UI start-up budget, the
//! 2k-node graph and the 2D solver's step do the same.
//!
//! So the parent of a timed body, **inside the exclusive lock** (no other timed body can start
//! meanwhile, and new GPU turns queue behind it), waits until the machine is quiet:
//!
//! * it samples the share of all cores busy **outside this process** over a window
//!   ([`QUIET_WINDOW`], ~1 s: Windows `GetSystemTimes` less this process's CPU time; Linux
//!   `/proc/stat` less `/proc/self/stat`), and while that share is above
//!   [`QUIET_THRESHOLD`] it sleeps [`QUIET_RECHECK`] and samples again;
//! * the wait is **bounded** ([`QUIET_MAX_WAIT`]; `FORGE_TIMED_QUIET_MAX_S` overrides it,
//!   `0` = sample once and never wait). When the bound expires the body runs anyway, and the
//!   report says it was **measured under load** — printed, logged, and appended to a failure
//!   (the result is still asserted: nothing is retried and no budget moves, W5);
//! * after a bound expired, the machine is known to be persistently busy (another lane's full
//!   test run, a game): for [`QUIET_EXPIRY_MEMORY`] afterwards the next waits are capped at
//!   [`QUIET_MAX_WAIT_AFTER_EXPIRY`], so a run of twenty timed bodies does not wait ten
//!   minutes twenty times;
//! * every wait is **observable**: one line per timed body (how long it waited, how many
//!   samples, the load it saw, the outcome) is printed with the test's output and appended to
//!   [`QUIET_LOG_FILE`] in the temp directory, and the process keeps [`quiet_counters`] and
//!   the [`last_quiet_report`].
//!
//! This is scheduling, not a tolerance: no budget is widened and nothing is retried (W5).
//!
//! Positive control (W2): [`QuietFaults::fake_load`] replaces the sampler with a fixed load
//! (`crates/forge-trace/tests/test_timed_quiet.rs`): a faked busy machine makes the gate wait
//! out its bound and report "MEASURED UNDER LOAD".

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The share of all cores busy outside this process above which the machine is not quiet.
pub const QUIET_THRESHOLD: f64 = 0.35;
/// How long one load sample measures.
pub const QUIET_WINDOW: Duration = Duration::from_secs(1);
/// How long a busy sample is followed by a pause before the next one.
pub const QUIET_RECHECK: Duration = Duration::from_secs(3);
/// The bound on one wait: after it the body runs anyway and is reported as under load.
pub const QUIET_MAX_WAIT: Duration = Duration::from_secs(600);
/// The bound on a wait that starts within [`QUIET_EXPIRY_MEMORY`] of an expired one.
pub const QUIET_MAX_WAIT_AFTER_EXPIRY: Duration = Duration::from_secs(60);
/// How long an expired bound shortens the waits after it.
pub const QUIET_EXPIRY_MEMORY: Duration = Duration::from_secs(900);
/// Overrides [`QUIET_MAX_WAIT`], in whole seconds (`0`: sample once, never wait).
pub const QUIET_MAX_ENV: &str = "FORGE_TIMED_QUIET_MAX_S";
/// One line per timed body's wait, appended in `std::env::temp_dir()` (kept under 1 MiB).
pub const QUIET_LOG_FILE: &str = "forge-timed-alone.log";
/// The time (Unix seconds) of the last wait whose bound expired, in the temp directory.
pub const QUIET_EXPIRY_FILE: &str = "forge-timed-alone.busy";

const LOG_CAP_BYTES: u64 = 1 << 20;

crate::control_switches! {
    /// W2 fault switches of the quiet wait (positive controls only).
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct QuietFaults {
        /// Report this share of all cores busy outside this process instead of sampling the
        /// machine (the sample window is still waited out): `Some(0.9)` fakes a busy machine,
        /// `Some(0.0)` an idle one. A faked wait never records an expiry for other waits.
        pub fake_load: Option<f64>,
    }
}

/// How a timed body waits for a quiet machine.
#[derive(Clone, Debug, PartialEq)]
pub struct QuietPolicy {
    /// Above this share of all cores busy outside this process, the machine is not quiet.
    pub threshold: f64,
    /// How long one sample measures.
    pub window: Duration,
    /// The pause after a busy sample.
    pub recheck: Duration,
    /// The bound on the whole wait.
    pub max_wait: Duration,
}

impl Default for QuietPolicy {
    fn default() -> Self {
        Self {
            threshold: QUIET_THRESHOLD,
            window: QUIET_WINDOW,
            recheck: QUIET_RECHECK,
            max_wait: QUIET_MAX_WAIT,
        }
    }
}

impl QuietPolicy {
    /// The policy a timed body waits under: the default, the bound from
    /// [`QUIET_MAX_ENV`] if set, capped at [`QUIET_MAX_WAIT_AFTER_EXPIRY`] when a bound
    /// expired within [`QUIET_EXPIRY_MEMORY`].
    #[must_use]
    pub fn for_timed_body() -> Self {
        let env = std::env::var(QUIET_MAX_ENV).ok();
        let since_expiry = last_expiry().and_then(|t| SystemTime::now().duration_since(t).ok());
        Self {
            max_wait: max_wait_under(env.as_deref(), since_expiry),
            ..Self::default()
        }
    }
}

/// The bound a wait gets: [`QUIET_MAX_WAIT`] or the env override (whole seconds; unreadable
/// values are ignored), capped when the last expiry is recent.
#[must_use]
pub fn max_wait_under(env: Option<&str>, since_last_expiry: Option<Duration>) -> Duration {
    let base = env
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map_or(QUIET_MAX_WAIT, Duration::from_secs);
    match since_last_expiry {
        Some(d) if d < QUIET_EXPIRY_MEMORY => base.min(QUIET_MAX_WAIT_AFTER_EXPIRY),
        _ => base,
    }
}

/// One load sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadSample {
    /// The share of all cores busy outside this process over the window, `0..=1`.
    pub outside: f64,
    /// The share of all cores busy, this process included, `0..=1`.
    pub system: f64,
    /// The share of all cores this process used, `0..=1`.
    pub own: f64,
    /// The window measured.
    pub window: Duration,
    /// The logical cores the shares are of (one core is `1 / cores` of the machine).
    pub cores: u32,
}

/// CPU time counters in one unit (100 ns ticks on Windows, clock ticks on Linux), summed
/// over every core.
#[derive(Clone, Copy, Debug)]
struct Counters {
    busy: u64,
    total: u64,
    own: u64,
    /// The logical cores the totals are summed over.
    cores: u32,
}

/// Samples how busy the machine is (see the module docs).
pub struct LoadSampler {
    backend: Backend,
}

impl std::fmt::Debug for LoadSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadSampler").finish_non_exhaustive()
    }
}

enum Backend {
    #[cfg(windows)]
    Helper(win::Helper),
    #[cfg(target_os = "linux")]
    Proc,
}

impl LoadSampler {
    /// A sampler of this machine. `Err` names why there is none (another OS, or the helper
    /// could not start).
    pub fn new() -> Result<Self, String> {
        #[cfg(windows)]
        {
            Ok(Self {
                backend: Backend::Helper(win::Helper::start()?),
            })
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                backend: Backend::Proc,
            })
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Err("no load sampler on this OS (Windows and Linux only, E-33)".into())
        }
    }

    fn read(&mut self) -> Result<Counters, String> {
        match &mut self.backend {
            #[cfg(windows)]
            Backend::Helper(h) => h.read(),
            #[cfg(target_os = "linux")]
            Backend::Proc => linux::read(),
        }
    }

    /// Measure the load over `window` (this thread sleeps through it).
    pub fn sample(&mut self, window: Duration) -> Result<LoadSample, String> {
        let a = self.read()?;
        std::thread::sleep(window);
        let b = self.read()?;
        shares(a, b, window)
    }
}

fn shares(a: Counters, b: Counters, window: Duration) -> Result<LoadSample, String> {
    let total = b.total.saturating_sub(a.total);
    if total == 0 {
        return Err("the system CPU time did not advance over the sample".into());
    }
    let t = total as f64;
    let system = (b.busy.saturating_sub(a.busy) as f64 / t).clamp(0.0, 1.0);
    let own = (b.own.saturating_sub(a.own) as f64 / t).clamp(0.0, 1.0);
    Ok(LoadSample {
        outside: (system - own).clamp(0.0, 1.0),
        system,
        own,
        window,
        cores: b.cores,
    })
}

#[cfg(windows)]
mod win {
    //! `GetSystemTimes` without `unsafe` in this crate (`#![forbid(unsafe_code)]`): a
    //! PowerShell helper (the same route `set_priority_class` takes) declares the call once
    //! and answers one line of counters per request, so a wait pays its start-up once.

    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use super::Counters;

    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -Namespace ForgeTimed -Name Cpu -MemberDefinition '[DllImport("kernel32.dll")] public static extern bool GetSystemTimes(out long idle, out long kernel, out long user);'
$p = [Diagnostics.Process]::GetProcessById(%PID%)
[long]$i = 0; [long]$k = 0; [long]$u = 0
[Console]::Out.WriteLine('ready')
[Console]::Out.Flush()
while ($null -ne [Console]::In.ReadLine()) {
  $p.Refresh()
  $ok = [ForgeTimed.Cpu]::GetSystemTimes([ref]$i, [ref]$k, [ref]$u)
  [Console]::Out.WriteLine("$ok $i $k $u $($p.TotalProcessorTime.Ticks)")
  [Console]::Out.Flush()
}
"#;

    pub(super) struct Helper {
        child: Child,
        stdin: Option<ChildStdin>,
        out: BufReader<ChildStdout>,
    }

    impl Helper {
        pub(super) fn start() -> Result<Self, String> {
            let script = SCRIPT.replace("%PID%", &std::process::id().to_string());
            let mut child = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &script,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("could not start the load sampler (powershell): {e}"))?;
            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the load sampler has no pipes".into());
            };
            let mut h = Self {
                child,
                stdin: Some(stdin),
                out: BufReader::new(stdout),
            };
            let mut line = String::new();
            match h.out.read_line(&mut line) {
                Ok(n) if n > 0 && line.trim() == "ready" => Ok(h),
                _ => Err(format!(
                    "the load sampler did not start (said {:?})",
                    line.trim()
                )),
            }
        }

        pub(super) fn read(&mut self) -> Result<Counters, String> {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| "the load sampler is closed".to_string())?;
            writeln!(stdin, "s")
                .and_then(|()| stdin.flush())
                .map_err(|e| format!("the load sampler stopped: {e}"))?;
            let mut line = String::new();
            self.out
                .read_line(&mut line)
                .map_err(|e| format!("the load sampler stopped: {e}"))?;
            parse(&line)
        }
    }

    fn parse(line: &str) -> Result<Counters, String> {
        let f: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> Result<u64, String> {
            f.get(i)
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| format!("the load sampler said {:?}", line.trim()))
        };
        if f.first() != Some(&"True") {
            return Err(format!("GetSystemTimes failed: {:?}", line.trim()));
        }
        let (idle, kernel, user, own) = (num(1)?, num(2)?, num(3)?, num(4)?);
        // Kernel time includes idle time (GetSystemTimes).
        let total = kernel.saturating_add(user);
        Ok(Counters {
            busy: total.saturating_sub(idle),
            total,
            own,
            cores: std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX)),
        })
    }

    impl Drop for Helper {
        fn drop(&mut self) {
            // Closing stdin ends the helper's loop; kill it too in case it is stuck. It is
            // our own child, killed by its handle (never by image name).
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::parse;

        #[test]
        fn a_helper_line_parses_and_idle_is_not_busy() {
            let c = parse("True 600 1000 200 50\n").expect("parses");
            assert_eq!((c.busy, c.total, c.own), (600, 1200, 50));
            assert!(parse("False 0 0 0 0").is_err());
            assert!(parse("garbage").is_err());
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    //! `/proc/stat`'s aggregate `cpu` line and `/proc/self/stat`'s `utime + stime`, both in
    //! clock ticks (`USER_HZ`).

    use super::Counters;

    pub(super) fn read() -> Result<Counters, String> {
        let stat = std::fs::read_to_string("/proc/stat").map_err(|e| format!("/proc/stat: {e}"))?;
        let own = std::fs::read_to_string("/proc/self/stat")
            .map_err(|e| format!("/proc/self/stat: {e}"))?;
        parse(&stat, &own)
    }

    pub(super) fn parse(stat: &str, own_stat: &str) -> Result<Counters, String> {
        let cpu = stat
            .lines()
            .find(|l| l.starts_with("cpu "))
            .ok_or("no aggregate cpu line in /proc/stat")?;
        let v: Vec<u64> = cpu
            .split_whitespace()
            .skip(1)
            .map(|s| s.parse::<u64>().unwrap_or(0))
            .collect();
        if v.len() < 4 {
            return Err(format!("short /proc/stat cpu line: {cpu:?}"));
        }
        // user nice system idle iowait irq softirq steal (guest time is already in user).
        let total: u64 = v.iter().take(8).sum();
        let idle = v[3] + v.get(4).copied().unwrap_or(0);
        // The command name may hold spaces and parentheses: fields resume after the last ')'.
        let rest: Vec<&str> = own_stat
            .rsplit_once(')')
            .map(|(_, r)| r.split_whitespace().collect())
            .unwrap_or_default();
        // After the name: state(3) ppid(4) ... utime(14) stime(15) -> indices 11 and 12.
        let tick = |i: usize| rest.get(i).and_then(|s| s.parse::<u64>().ok());
        let own = match (tick(11), tick(12)) {
            (Some(u), Some(s)) => u + s,
            _ => return Err(format!("unreadable /proc/self/stat: {own_stat:?}")),
        };
        // The per-core lines (`cpu0`, `cpu1`, ...): the cores the aggregate sums over (the
        // whole machine or VM, not a container's CPU quota).
        let cores = stat
            .lines()
            .filter(|l| l.starts_with("cpu") && !l.starts_with("cpu "))
            .count();
        Ok(Counters {
            busy: total.saturating_sub(idle),
            total,
            own,
            cores: u32::try_from(cores.max(1)).unwrap_or(u32::MAX),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::parse;

        #[test]
        fn proc_stat_lines_parse() {
            let stat = "cpu  100 5 50 800 20 3 2 0 0 0\ncpu0 1 1 1 1 1 1 1 1 1 1\n";
            let own = "1234 (a (b) c) R 1 2 3 4 5 6 7 8 9 10 30 12 0 0\n";
            let c = parse(stat, own).expect("parses");
            assert_eq!(c.total, 980);
            assert_eq!(c.busy, 160);
            assert_eq!(c.own, 42);
            assert_eq!(c.cores, 1);
        }
    }
}

/// How a wait ended.
#[derive(Clone, Debug, PartialEq)]
pub enum QuietOutcome {
    /// The load fell to the threshold: the body measures on a quiet machine.
    Quiet,
    /// The bound expired with the machine still busy: the body measures under load.
    UnderLoad,
    /// The load could not be sampled (named): the body runs without waiting.
    Unmeasured(String),
}

/// What one wait saw (see [`wait_for_quiet`]).
#[derive(Clone, Debug, PartialEq)]
pub struct QuietReport {
    /// How the wait ended.
    pub outcome: QuietOutcome,
    /// How long it took, samples included.
    pub waited: Duration,
    /// The samples taken.
    pub samples: u32,
    /// The first sample's load outside this process (`None`: no sample).
    pub first: Option<f64>,
    /// The highest sampled load outside this process.
    pub peak: Option<f64>,
    /// The last sampled load outside this process.
    pub last: Option<f64>,
    /// The threshold it waited for.
    pub threshold: f64,
    /// The bound it waited under.
    pub max_wait: Duration,
    /// Whether the load was faked (a positive control).
    pub faked: bool,
}

impl QuietReport {
    /// Whether the body measured under load (the bound expired).
    #[must_use]
    pub fn under_load(&self) -> bool {
        self.outcome == QuietOutcome::UnderLoad
    }

    /// The report's one line, for the test output and the log.
    #[must_use]
    pub fn line(&self, test: &str) -> String {
        let pct = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{:.0}%", v * 100.0));
        let outcome = match &self.outcome {
            QuietOutcome::Quiet => "quiet".to_string(),
            QuietOutcome::UnderLoad => "MEASURED UNDER LOAD (the bound expired with the machine \
                                        still busy; the result is still asserted)"
                .to_string(),
            QuietOutcome::Unmeasured(why) => {
                format!("load unmeasured ({why}); ran without waiting")
            }
        };
        format!(
            "forge-timed-alone: {test}: waited {:.1} s for a quiet machine ({} sample(s) of the \
             load outside this process: first {}, peak {}, last {}; threshold {:.0}%, bound {:.1} s{}): {outcome}",
            self.waited.as_secs_f64(),
            self.samples,
            pct(self.first),
            pct(self.peak),
            pct(self.last),
            self.threshold * 100.0,
            self.max_wait.as_secs_f64(),
            if self.faked {
                ", load FAKED (control)"
            } else {
                ""
            },
        )
    }
}

/// Wait until the machine is quiet under `policy` (see the module docs). Never fails: a load
/// that cannot be sampled is reported as [`QuietOutcome::Unmeasured`] and not waited for.
pub fn wait_for_quiet(policy: &QuietPolicy, faults: QuietFaults) -> QuietReport {
    let start = Instant::now();
    let fake = faults.fake_load();
    let mut report = QuietReport {
        outcome: QuietOutcome::Quiet,
        waited: Duration::ZERO,
        samples: 0,
        first: None,
        peak: None,
        last: None,
        threshold: policy.threshold,
        max_wait: policy.max_wait,
        faked: fake.is_some(),
    };
    let mut sampler = None;
    if fake.is_none() {
        match LoadSampler::new() {
            Ok(s) => sampler = Some(s),
            Err(e) => {
                report.outcome = QuietOutcome::Unmeasured(e);
                report.waited = start.elapsed();
                return report;
            }
        }
    }
    loop {
        let sample = match (&mut sampler, fake) {
            (_, Some(load)) => {
                std::thread::sleep(policy.window);
                Ok(load.clamp(0.0, 1.0))
            }
            (Some(s), None) => s.sample(policy.window).map(|s| s.outside),
            (None, None) => Err("no sampler".to_string()),
        };
        let load = match sample {
            Ok(l) => l,
            Err(e) => {
                report.outcome = QuietOutcome::Unmeasured(e);
                break;
            }
        };
        report.samples += 1;
        report.first.get_or_insert(load);
        report.peak = Some(report.peak.map_or(load, |p| p.max(load)));
        report.last = Some(load);
        if load <= policy.threshold {
            report.outcome = QuietOutcome::Quiet;
            break;
        }
        // Another pause and sample must fit in the bound, or the bound has expired.
        if start.elapsed() + policy.recheck + policy.window > policy.max_wait {
            report.outcome = QuietOutcome::UnderLoad;
            break;
        }
        std::thread::sleep(policy.recheck);
    }
    report.waited = start.elapsed();
    report
}

static WAITS: AtomicU64 = AtomicU64::new(0);
static WAITED_MS: AtomicU64 = AtomicU64::new(0);
static UNDER_LOAD: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static LAST: std::cell::RefCell<Option<QuietReport>> = const { std::cell::RefCell::new(None) };
}

/// This process's quiet waits so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuietCounters {
    /// Waits made (one per timed body).
    pub waits: u64,
    /// Milliseconds waited in all.
    pub waited_ms: u64,
    /// Waits whose bound expired (bodies measured under load).
    pub under_load: u64,
}

/// This process's quiet waits so far.
#[must_use]
pub fn quiet_counters() -> QuietCounters {
    QuietCounters {
        waits: WAITS.load(Ordering::Relaxed),
        waited_ms: WAITED_MS.load(Ordering::Relaxed),
        under_load: UNDER_LOAD.load(Ordering::Relaxed),
    }
}

/// The last wait made on this thread (a timed test waits on its own libtest thread).
#[must_use]
pub fn last_quiet_report() -> Option<QuietReport> {
    LAST.with(|l| l.borrow().clone())
}

/// A timed body's wait: [`wait_for_quiet`], then counted, printed with the test's output,
/// appended to [`QUIET_LOG_FILE`], and (a real, expired bound) recorded for the waits after
/// it. Call it while holding the machine lock: the log and the expiry file are written under
/// it.
pub fn wait_for_quiet_reported(
    test: &str,
    policy: &QuietPolicy,
    faults: QuietFaults,
) -> QuietReport {
    let report = wait_for_quiet(policy, faults);
    WAITS.fetch_add(1, Ordering::Relaxed);
    WAITED_MS.fetch_add(
        u64::try_from(report.waited.as_millis()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    if report.under_load() {
        UNDER_LOAD.fetch_add(1, Ordering::Relaxed);
        if !report.faked {
            record_expiry();
        }
    }
    let line = report.line(test);
    println!("{line}");
    append_log(&line);
    LAST.with(|l| *l.borrow_mut() = Some(report.clone()));
    report
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn append_log(line: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join(QUIET_LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_CAP_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{} pid {} {line}", unix_now(), std::process::id());
    }
}

fn record_expiry() {
    let _ = std::fs::write(
        std::env::temp_dir().join(QUIET_EXPIRY_FILE),
        unix_now().to_string(),
    );
}

fn last_expiry() -> Option<SystemTime> {
    let s = std::fs::read_to_string(std::env::temp_dir().join(QUIET_EXPIRY_FILE)).ok()?;
    let secs = s.trim().parse::<u64>().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_comes_from_the_env_and_shrinks_after_an_expiry() {
        assert_eq!(max_wait_under(None, None), QUIET_MAX_WAIT);
        assert_eq!(max_wait_under(Some("30"), None), Duration::from_secs(30));
        assert_eq!(max_wait_under(Some("0"), None), Duration::ZERO);
        assert_eq!(max_wait_under(Some("soon"), None), QUIET_MAX_WAIT);
        assert_eq!(
            max_wait_under(None, Some(Duration::from_secs(10))),
            QUIET_MAX_WAIT_AFTER_EXPIRY
        );
        assert_eq!(
            max_wait_under(Some("20"), Some(Duration::from_secs(10))),
            Duration::from_secs(20)
        );
        assert_eq!(
            max_wait_under(None, Some(QUIET_EXPIRY_MEMORY + Duration::from_secs(1))),
            QUIET_MAX_WAIT
        );
    }

    #[test]
    fn shares_split_own_from_outside() {
        let a = Counters {
            busy: 100,
            total: 1000,
            own: 10,
            cores: 4,
        };
        let b = Counters {
            busy: 600,
            total: 2000,
            own: 110,
            cores: 4,
        };
        let s = shares(a, b, Duration::from_secs(1)).expect("advanced");
        assert!((s.system - 0.5).abs() < 1e-12);
        assert!((s.own - 0.1).abs() < 1e-12);
        assert!((s.outside - 0.4).abs() < 1e-12);
        assert!(shares(a, a, Duration::ZERO).is_err());
    }

    #[test]
    fn a_zero_bound_samples_once_and_does_not_wait() {
        let p = QuietPolicy {
            max_wait: Duration::ZERO,
            window: Duration::from_millis(10),
            recheck: Duration::from_secs(60),
            ..QuietPolicy::default()
        };
        let r = wait_for_quiet(
            &p,
            QuietFaults {
                fake_load: Some(1.0),
            },
        );
        assert_eq!(r.samples, 1);
        assert!(r.under_load());
        assert!(r.waited < Duration::from_secs(30), "{:?}", r.waited);
    }
}
