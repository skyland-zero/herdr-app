//! CPU and memory of this computer, or of one the person is connected to.
//!
//! The sidebar asks often, so an ask is always answered from the cache: a look that
//! is recent enough starts nothing, and only one look per machine runs at a time (a
//! second ask while one runs gets the last answer instead of queueing).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui_shell::{HostError, HostValue};

/// One look at a machine: monotonic CPU ticks (their unit is the machine's
/// business) and memory in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raw {
    pub busy: u64,
    pub total: u64,
    pub used: u64,
    pub capacity: u64,
    pub cores: u32,
}

#[derive(Default)]
struct Entry {
    /// The newest look, and when it arrived.
    last: Option<(Instant, Raw)>,
    /// The look before it: CPU is the change between the two.
    prev: Option<Raw>,
    error: Option<String>,
    /// A look is running; asks meanwhile answer from the cache.
    busy: bool,
    /// When the last look started: nothing new before `min_gap`.
    started: Option<Instant>,
    /// How long to wait before trying again after a failure (2 s, doubling to 30).
    wait: Duration,
    /// A machine away with no `sh` (Windows): its looks cost a program start.
    remote_windows: bool,
    /// The sampler placed there, and where. Placed once, then only started.
    windows_script: Option<String>,
    /// That machine would not run the sampler: ask PowerShell instead.
    no_cscript: bool,
}

static CACHE: std::sync::LazyLock<Mutex<HashMap<Option<String>, Entry>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The shortest gap between two looks. Here, reading counters costs microseconds;
/// on a machine away it starts a program there — worth it twice a second on Unix.
/// Windows starts the small sampler `WINDOWS_SAMPLER` places (`cscript`: a tenth of
/// the CPU of a PowerShell start, and three counter reads instead of one), so it is
/// only a little slower than Unix there.
fn min_gap(on: Option<&str>, remote_windows: bool) -> Duration {
    match (on, remote_windows) {
        (None, _) => Duration::from_millis(1000),
        (Some(_), false) => Duration::from_millis(2000),
        (Some(_), true) => Duration::from_millis(1500),
    }
}

/// CPU and memory, and how old the numbers are. `on`: that machine (a space's
/// `machine`). Answers from the cache when it can: most asks start nothing, and
/// one that does never waits for another.
pub fn stats(on: Option<String>) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    type Answer = std::pin::Pin<Box<dyn std::future::Future<Output = Result<HostValue, HostError>> + Send>>;
    if let Some(value) = claim(&on) {
        return Box::pin(std::future::ready(Ok(value))) as Answer;
    }
    // A look reads files or starts a program on the machine: off this thread.
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(finish(on));
    });
    Box::pin(async move { rx.recv().await.map_err(|e| HostError::new(e.to_string()))? }) as Answer
}

/// The numbers already had, or `None` when this ask must take the look (and has
/// marked one running).
fn claim(on: &Option<String>) -> Option<HostValue> {
    let now = Instant::now();
    let mut all = CACHE.lock().unwrap();
    let entry = all.entry(on.clone()).or_default();
    let gap = entry.wait.max(min_gap(on.as_deref(), entry.remote_windows));
    if entry.busy || entry.started.is_some_and(|at| now.duration_since(at) < gap) {
        return Some(report(entry));
    }
    entry.busy = true;
    entry.started = Some(now);
    None
}

fn finish(on: Option<String>) -> Result<HostValue, HostError> {
    let (windows, script, no_cscript) = {
        let all = CACHE.lock().unwrap();
        match all.get(&on) {
            Some(entry) => (entry.remote_windows, entry.windows_script.clone(), entry.no_cscript),
            None => (false, None, false),
        }
    };
    let looked = match on.as_deref() {
        None => Looked { raw: look_here(), windows: None, script: None, no_cscript: false },
        Some(machine) => look_there(machine, windows, script, no_cscript),
    };
    let mut all = CACHE.lock().unwrap();
    let entry = all.entry(on).or_default();
    entry.busy = false;
    if let Some(windows) = looked.windows {
        entry.remote_windows = windows;
    }
    if let Some(script) = looked.script {
        entry.windows_script = script;
    }
    if looked.no_cscript {
        entry.no_cscript = true;
    }
    match looked.raw {
        Ok(raw) => {
            entry.prev = entry.last.map(|(_, raw)| raw);
            entry.last = Some((Instant::now(), raw));
            entry.error = None;
            entry.wait = Duration::ZERO;
        }
        Err(why) => {
            entry.error = Some(why);
            entry.wait = if entry.wait.is_zero() { Duration::from_secs(2) } else { (entry.wait * 2).min(Duration::from_secs(30)) };
        }
    }
    Ok(report(entry))
}

fn report(entry: &Entry) -> HostValue {
    let cpu = match (entry.prev, entry.last) {
        (Some(before), Some((_, now))) => {
            let ticks = now.total.saturating_sub(before.total);
            let busy = now.busy.saturating_sub(before.busy);
            (ticks > 0).then(|| (busy as f64 / ticks as f64).clamp(0.0, 1.0))
        }
        _ => None,
    };
    let raw = entry.last.map(|(_, raw)| raw);
    let memory = raw.filter(|raw| raw.capacity > 0).map(|raw| (raw.used as f64 / raw.capacity as f64).clamp(0.0, 1.0));
    let age = entry.last.map(|(at, _)| at.elapsed().as_millis() as u64);
    crate::mods::to_host(&serde_json::json!({
        "cpu": cpu,
        "mem": memory,
        "memTotal": raw.map(|raw| raw.capacity).filter(|bytes| *bytes > 0),
        "cores": raw.map(|raw| raw.cores).filter(|cores| *cores > 0),
        "age": age,
        "error": entry.error,
    }))
}

/// This computer, starting nothing.
fn look_here() -> Result<Raw, String> {
    #[cfg(target_os = "linux")]
    {
        linux()
    }
    #[cfg(target_os = "macos")]
    {
        macos()
    }
    #[cfg(windows)]
    {
        windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err("this system keeps no CPU counters Herdr can read".into())
    }
}

#[cfg(target_os = "linux")]
fn linux() -> Result<Raw, String> {
    let stat = std::fs::read_to_string("/proc/stat").map_err(|e| format!("/proc/stat: {e}"))?;
    let line = stat.lines().next().ok_or("/proc/stat is empty")?;
    let ticks: Vec<u64> = line.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal [guest guest_nice]: the last two
    // are already counted in user and nice, so they are left out.
    if ticks.len() < 4 {
        return Err("cannot read /proc/stat".into());
    }
    let idle = ticks[3] + ticks.get(4).copied().unwrap_or(0);
    let total: u64 = ticks.iter().take(8).sum();
    let mem = std::fs::read_to_string("/proc/meminfo").map_err(|e| format!("/proc/meminfo: {e}"))?;
    let field = |key: &str| -> Option<u64> {
        let line = mem.lines().find(|l| l.starts_with(key))?;
        let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kilobytes * 1024)
    };
    let capacity = field("MemTotal:").unwrap_or(0);
    let available = field("MemAvailable:").or_else(|| field("MemFree:")).unwrap_or(0);
    Ok(Raw { busy: total - idle, total, used: capacity.saturating_sub(available), capacity, cores: cores() })
}

#[cfg(windows)]
fn windows() -> Result<Raw, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return Err("GetSystemTimes failed".into());
    }
    // Kernel time includes idle time, so the total is kernel plus user alone.
    let total = ticks(kernel) + ticks(user);
    let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    memory.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return Err("GlobalMemoryStatusEx failed".into());
    }
    Ok(Raw {
        busy: total.saturating_sub(ticks(idle)),
        total,
        used: memory.ullTotalPhys.saturating_sub(memory.ullAvailPhys),
        capacity: memory.ullTotalPhys,
        cores: cores(),
    })
}

#[cfg(target_os = "macos")]
fn macos() -> Result<Raw, String> {
    // `vm_stat` would be simpler, but that is a program start on every look; mach
    // has the same numbers in memory.
    let host = unsafe { libc::mach_host_self() };
    let mut load: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    let result = unsafe {
        libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, &mut load as *mut _ as *mut libc::integer_t, &mut count)
    };
    if result != libc::KERN_SUCCESS {
        return Err("host_statistics(HOST_CPU_LOAD_INFO) failed".into());
    }
    let ticks = &load.cpu_ticks;
    let at = |state: libc::c_int| ticks[state as usize] as u64;
    let busy = at(libc::CPU_STATE_USER) + at(libc::CPU_STATE_SYSTEM) + at(libc::CPU_STATE_NICE);
    let total = busy + at(libc::CPU_STATE_IDLE);

    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let result = unsafe {
        libc::host_statistics64(host, libc::HOST_VM_INFO64, &mut vm as *mut _ as *mut libc::integer_t, &mut count)
    };
    if result != libc::KERN_SUCCESS {
        return Err("host_statistics64(HOST_VM_INFO64) failed".into());
    }
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    let capacity = sysctl_u64("hw.memsize").unwrap_or(0);
    // Free, and pages the system would give up under pressure: the rest is in use.
    let free = (vm.free_count as u64 + vm.inactive_count as u64 + vm.speculative_count as u64) * page;
    Ok(Raw { busy, total, used: capacity.saturating_sub(free), capacity, cores: cores() })
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let ok = unsafe {
        libc::sysctlbyname(name.as_ptr(), &mut value as *mut _ as *mut std::ffi::c_void, &mut size, std::ptr::null_mut(), 0)
    };
    (ok == 0).then_some(value)
}

fn cores() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0)
}

/// One program run on a machine away that prints everything a look needs, whichever
/// Unix it is: /proc on Linux, the mach counters on macOS.
const UNIX_SCRIPT: &str = "if [ -r /proc/stat ]; then head -1 /proc/stat; grep -E '^(MemTotal|MemAvailable):' /proc/meminfo; nproc; else sysctl -n kern.cp_time; sysctl -n hw.memsize; sysctl -n hw.ncpu; vm_stat; fi";

/// The same for Windows, through PowerShell: the raw performance counters (only a
/// difference between two reads means anything), and the memory in use. The last
/// resort: used where no sampler can be kept (see `place_sampler`).
const WINDOWS_SCRIPT: &str = r#"$p=Get-CimInstance Win32_PerfRawData_PerfOS_Processor -Filter "Name='_Total'"; $m=Get-CimInstance Win32_PerfRawData_PerfOS_Memory; $o=Get-CimInstance Win32_OperatingSystem; '{0} {1} {2} {3} {4}' -f $p.PercentIdleTime,$p.TimeStamp_Sys100NS,[long]$m.AvailableBytes,[long]$o.TotalVisibleMemorySize*1024,[int]$o.NumberOfLogicalProcessors"#;

/// The same for Windows: a tiny script, placed there once and then only started.
/// `cscript` costs a tenth of the CPU of a PowerShell start (31 ms against 312 ms
/// measured), so a Windows machine away stays nearly as fresh as a Unix one.
/// Raw performance counters only: a difference between two reads means something.
const WINDOWS_SAMPLER: &str = r#"Dim o,p,m,c,s
Set o = GetObject("winmgmts:\\.\root\cimv2")
For Each p In o.ExecQuery("SELECT PercentIdleTime,TimeStamp_Sys100NS FROM Win32_PerfRawData_PerfOS_Processor WHERE Name='_Total'")
  s = p.PercentIdleTime & " " & p.TimeStamp_Sys100NS
Next
For Each m In o.ExecQuery("SELECT AvailableBytes FROM Win32_PerfRawData_PerfOS_Memory")
  s = s & " " & m.AvailableBytes
Next
For Each c In o.ExecQuery("SELECT TotalVisibleMemorySize FROM Win32_OperatingSystem")
  s = s & " " & (c.TotalVisibleMemorySize * 1024)
Next
WScript.Echo s & " " & CreateObject("WScript.Shell").Environment("PROCESS")("NUMBER_OF_PROCESSORS")
"#;

/// What a look there came to, and what it taught about the machine.
struct Looked {
    raw: Result<Raw, String>,
    /// Whether it has a shell (`sh`): what tells Windows apart from Unix there.
    windows: Option<bool>,
    /// The sampler's path, once it is known.
    script: Option<Option<String>>,
    /// No `cscript` there: ask PowerShell from now on.
    no_cscript: bool,
}

fn look_there(machine: &str, windows: bool, script: Option<String>, no_cscript: bool) -> Looked {
    let settled = |raw| Looked { raw, windows: None, script: None, no_cscript: false };
    if !windows {
        let args = ["-c".to_owned(), UNIX_SCRIPT.to_owned()];
        return match crate::mods::run_remote(machine, "sh", &args, None) {
            // No `sh` there: Windows, and nothing learned about it yet.
            Err(why) if why.contains("cannot start") => Looked { raw: Err(why), windows: Some(true), script: None, no_cscript: false },
            Err(why) => settled(Err(why)),
            Ok((0, out, _)) => settled(match parse_unix(&String::from_utf8_lossy(&out)) {
                Some(raw) => Ok(raw),
                None => Err(UNREADABLE.into()),
            }),
            Ok((_, _, err)) => settled(Err(message(&err, UNREADABLE))),
        };
    }
    if !no_cscript {
        // Placed once: a PowerShell start costs six times the look itself.
        let path = match script {
            Some(path) => Some(path),
            None => place_sampler(machine),
        };
        if let Some(path) = path {
            let args = ["//nologo".to_owned(), path.clone()];
            match crate::mods::run_remote(machine, "cscript", &args, None) {
                Ok((0, out, _)) => {
                    let raw = parse_windows(&String::from_utf8_lossy(&out)).ok_or_else(|| UNREADABLE.to_owned());
                    return Looked { raw, windows: Some(true), script: Some(Some(path)), no_cscript: false };
                }
                // The script itself printed nothing usable: it may have been removed
                // (a cleaner emptied TEMP), and saying why is localized. Place it
                // again next look rather than trust the path.
                Ok((_, out, err)) => {
                    let said = message(&err, "");
                    let why = if said.is_empty() { message(&out, UNREADABLE) } else { said };
                    return Looked { raw: Err(why), windows: Some(true), script: Some(None), no_cscript: false };
                }
                Err(why) => {
                    // No `cscript` there: PowerShell from now on. Any other failure
                    // is this look's answer, and the next look tries the sampler.
                    let missing = why.contains("cannot start");
                    return Looked { raw: Err(why), windows: Some(true), script: Some(None), no_cscript: missing };
                }
            }
        }
    }
    // No place to keep a script, or no `cscript` to run one: PowerShell, in one go.
    let args = ["-NoProfile".to_owned(), "-NonInteractive".to_owned(), "-Command".to_owned(), WINDOWS_SCRIPT.to_owned()];
    match crate::mods::run_remote(machine, "powershell", &args, None) {
        Ok((0, out, _)) => Looked {
            raw: parse_windows(&String::from_utf8_lossy(&out)).ok_or_else(|| UNREADABLE.to_owned()),
            windows: Some(true),
            script: None,
            no_cscript: true,
        },
        Ok((_, _, err)) => Looked { raw: Err(message(&err, UNREADABLE)), windows: Some(true), script: None, no_cscript: true },
        Err(why) => Looked { raw: Err(why), windows: Some(true), script: None, no_cscript: true },
    }
}

/// Writes the sampler into that machine's TEMP, and answers where it landed.
fn place_sampler(machine: &str) -> Option<String> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(WINDOWS_SAMPLER.as_bytes());
    let command = format!(
        "$p=Join-Path $env:TEMP 'herdr-stats.vbs'; [IO.File]::WriteAllBytes($p,[Convert]::FromBase64String('{encoded}')); Write-Output $p"
    );
    let args = ["-NoProfile".to_owned(), "-NonInteractive".to_owned(), "-Command".to_owned(), command];
    match crate::mods::run_remote(machine, "powershell", &args, None) {
        Ok((0, out, _)) => {
            let path = String::from_utf8_lossy(&out).trim().to_owned();
            (!path.is_empty()).then_some(path)
        }
        _ => None,
    }
}

const UNREADABLE: &str = "cannot read its CPU and memory";

fn message(err: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(err).trim().to_owned();
    if text.is_empty() { fallback.to_owned() } else { text }
}

/// What `UNIX_SCRIPT` printed, on either Unix.
fn parse_unix(text: &str) -> Option<Raw> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next()?;
    if first.starts_with("cpu") {
        // Linux: /proc/stat's first line, then MemTotal, MemAvailable, nproc.
        let ticks: Vec<u64> = first.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
        if ticks.len() < 4 {
            return None;
        }
        let idle = ticks[3] + ticks.get(4).copied().unwrap_or(0);
        let total: u64 = ticks.iter().take(8).sum();
        let (mut capacity, mut available, mut seen) = (0u64, 0u64, 0u8);
        let mut cores = 0u32;
        for line in lines {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                capacity = rest.split_whitespace().next()?.parse::<u64>().ok()? * 1024;
                seen |= 1;
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available = rest.split_whitespace().next()?.parse::<u64>().ok()? * 1024;
                seen |= 2;
            } else if let Ok(n) = line.trim().parse() {
                cores = n;
            }
        }
        if seen != 3 {
            return None;
        }
        return Some(Raw { busy: total - idle, total, used: capacity.saturating_sub(available), capacity, cores });
    }
    // macOS: kern.cp_time (user nice sys idle intr), hw.memsize, hw.ncpu, vm_stat.
    let ticks: Vec<u64> = first.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    if ticks.len() < 4 {
        return None;
    }
    let capacity = lines.next()?.trim().parse::<u64>().ok()?;
    let cores = lines.next()?.trim().parse::<u32>().ok()?;
    let pages = |key: &str| -> Option<u64> {
        text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?;
            rest.trim_start_matches(':').trim().trim_end_matches('.').trim().parse().ok()
        })
    };
    // vm_stat prints its page size in the header; a look without it assumes Apple's.
    let page = text.lines().find_map(|line| {
        let rest = line.split("page size of").nth(1)?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    let free = (pages("Pages free")? + pages("Pages inactive")? + pages("Pages speculative")?) * page;
    Some(Raw {
        busy: ticks[0] + ticks[1] + ticks[2],
        total: ticks[0] + ticks[1] + ticks[2] + ticks[3],
        used: capacity.saturating_sub(free),
        capacity,
        cores,
    })
}

/// What `WINDOWS_SCRIPT` or `WINDOWS_SAMPLER` printed: idle ticks, timestamp, free
/// bytes, total bytes, cores.
fn parse_windows(text: &str) -> Option<Raw> {
    let fields: Vec<u64> = text.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    if fields.len() < 5 {
        return None;
    }
    let (idle, stamp, available, capacity, cores) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    Some(Raw {
        // The idle counter is cumulative: its change over the timestamp's change is
        // the idle share, which the difference between two looks turns around.
        busy: stamp.saturating_sub(idle),
        total: stamp,
        used: capacity.saturating_sub(available),
        capacity,
        cores: cores as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_counters_become_busy_and_used() {
        let text = "cpu  100 0 100 700 100 0 0 0 0 0\nMemTotal:       1000 kB\nMemAvailable:    250 kB\n8\n";
        let raw = parse_unix(text).unwrap();
        // Everything but idle and iowait is busy: 100 + 100 of 1000.
        assert_eq!((raw.busy, raw.total), (200, 1000));
        assert_eq!((raw.used, raw.capacity), (750 * 1024, 1000 * 1024));
        assert_eq!(raw.cores, 8);
    }

    #[test]
    fn macos_counters_read_vm_stat() {
        let text = "10 2 30 400 5\n17179869184\n10\nMach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                        100.\nPages active:                     5000.\nPages inactive:                   1000.\nPages speculative:                 200.\n";
        let raw = parse_unix(text).unwrap();
        assert_eq!((raw.busy, raw.total), (42, 442));
        assert_eq!(raw.cores, 10);
        assert_eq!(raw.capacity, 17179869184);
        // 1300 of 1048576 pages free: next to nothing, so nearly all of it is used.
        assert_eq!(raw.used, 17179869184 - 1300 * 16384);
    }

    #[test]
    fn windows_counters_subtract_the_cumulative_idle() {
        let raw = parse_windows("90000000 134383486901927271 8000000000 34000000000 40").unwrap();
        assert_eq!(raw.total, 134383486901927271);
        assert_eq!(raw.busy, 134383486901927271 - 90000000);
        assert_eq!((raw.used, raw.capacity), (26000000000, 34000000000));
        assert_eq!(raw.cores, 40);
    }

    #[test]
    fn the_windows_sampler_is_one_it_can_run() {
        // Placed as bytes and run by `cscript`: plain ASCII, no line longer than a
        // command line, no `%` or `"` that a shell would eat on the way.
        assert!(WINDOWS_SAMPLER.is_ascii());
        assert!(WINDOWS_SAMPLER.lines().all(|line| line.len() < 200));
        assert!(WINDOWS_SAMPLER.contains("Win32_PerfRawData_PerfOS_Processor"));
        // The same fields `parse_windows` reads, in the same order.
        let after = WINDOWS_SAMPLER.find("PercentIdleTime").unwrap();
        let memory = WINDOWS_SAMPLER.find("AvailableBytes").unwrap();
        let total = WINDOWS_SAMPLER.find("TotalVisibleMemorySize").unwrap();
        let cores = WINDOWS_SAMPLER.find("NUMBER_OF_PROCESSORS").unwrap();
        // `TotalVisibleMemorySize` is kilobytes, and not on the performance class:
        // asking `Win32_PerfRawData_PerfOS_Memory` for it fails the whole query.
        assert!(WINDOWS_SAMPLER.contains("Win32_OperatingSystem"));
        assert!(WINDOWS_SAMPLER.contains("TotalVisibleMemorySize * 1024"));
        assert!(after < memory && memory < total && total < cores);
    }

    #[cfg(windows)]
    #[test]
    fn the_sampler_runs_on_this_windows() {
        // The very file placed on a machine away, run here: a mistake in it would
        // otherwise only ever show up on someone else's computer.
        let path = std::env::temp_dir().join("herdr-stats-sampler-test.vbs");
        std::fs::write(&path, WINDOWS_SAMPLER).unwrap();
        let out = std::process::Command::new("cscript").arg("//nologo").arg(&path).output().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(out.status.success(), "cscript said: {}", String::from_utf8_lossy(&out.stdout));
        let raw = parse_windows(&String::from_utf8_lossy(&out.stdout)).expect("five numbers");
        assert!(raw.total > 0, "{raw:?}");
        assert!(raw.busy <= raw.total, "{raw:?}");
        assert!(raw.capacity > raw.used, "{raw:?}");
        assert!(raw.cores > 0, "{raw:?}");
    }

    #[test]
    fn cpu_is_the_change_between_two_looks() {
        let entry = Entry {
            prev: Some(Raw { busy: 100, total: 1000, used: 0, capacity: 0, cores: 4 }),
            last: Some((Instant::now(), Raw { busy: 400, total: 2000, used: 0, capacity: 0, cores: 4 })),
            ..Entry::default()
        };
        let value = report(&entry);
        // 300 of 1000 ticks more: 30% — not the 20% the newest look alone suggests.
        assert!(format!("{value:?}").contains("0.3"), "{value:?}");
    }

    #[test]
    fn this_computer_answers_with_numbers() {
        let raw = look_here().unwrap();
        assert!(raw.total > 0, "{raw:?}");
        assert!(raw.busy <= raw.total, "{raw:?}");
        assert!(raw.capacity > raw.used, "{raw:?}");
        assert!(raw.cores > 0, "{raw:?}");
    }

    #[test]
    fn a_look_right_after_another_answers_from_the_cache() {
        let answer = |on: Option<String>| match claim(&on) {
            Some(value) => Ok(value),
            None => finish(on),
        };
        let first = answer(None).unwrap();
        assert!(format!("{first:?}").contains("cpu"));
        let waited = Instant::now();
        answer(None).unwrap();
        assert!(waited.elapsed() < Duration::from_millis(500));
        assert_eq!(CACHE.lock().unwrap().len(), 1);
    }
}
