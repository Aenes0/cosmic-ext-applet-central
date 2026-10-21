// SPDX-License-Identifier: GPL-3.0-only

//! What the system says about itself, read from /proc and /sys, and from
//! NVIDIA's own library for its cards: counters, which only mean something
//! against the reading before (CPU time, bytes read, energy), and values as
//! they are now (memory, temperatures).

use std::{
    collections::HashMap,
    ffi::{CStr, c_char, c_void},
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::Instant,
};

/// Busy and all time of a CPU, in ticks.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    /// The page cache and what the kernel can give back.
    pub cache: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// zram: what's in it, and what that takes compressed.
    pub zram: Option<(u64, u64)>,
}

/// A temperature, °C, with when it's too hot if the chip says.
#[derive(Clone, Debug)]
pub struct Temp {
    pub label: String,
    pub celsius: f32,
    pub critical: Option<f32>,
}

/// A chip's sensors: the CPU's, a drive's, the network card's…
#[derive(Clone, Debug)]
pub struct Chip {
    /// k10temp, nvme, amdgpu…
    pub kind: String,
    /// The drive's model, for a drive; else empty.
    pub model: String,
    /// The block device it is, for a drive.
    pub disk: Option<String>,
    pub temps: Vec<Temp>,
    /// Fans, in RPM.
    pub fans: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub name: String,
    /// Percent.
    pub usage: f32,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temp: Option<f32>,
    pub watts: Option<f32>,
    pub watts_limit: Option<f32>,
    /// MHz.
    pub clock: Option<u32>,
    /// Percent.
    pub fan: Option<u32>,
    /// Since the driver loaded, in millijoules: for NVIDIA's.
    pub energy_mj: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Mount {
    pub path: String,
    pub device: String,
    pub total: u64,
    pub used: u64,
}

/// A process, as /proc has it.
#[derive(Clone, Debug)]
pub struct Proc {
    /// Its program, which its helpers share: a browser and its tabs.
    pub name: String,
    pub ticks: u64,
    /// Its own memory, not shared with others.
    pub memory: u64,
    pub mine: bool,
}

/// What doesn't change while running.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub cpu_model: String,
    pub threads: usize,
    pub cores: usize,
    /// Each drive's model, by block device.
    pub disk_models: Vec<(String, String)>,
    pub hostname: String,
    pub os: String,
    pub kernel: String,
}

/// A reading of everything.
#[derive(Clone, Debug)]
pub struct Reading {
    pub at: Instant,
    /// All CPUs first, then each.
    pub cpu: Vec<CpuTimes>,
    /// MHz: the average and the highest.
    pub frequency: Option<(f32, f32)>,
    pub memory: Memory,
    pub chips: Vec<Chip>,
    pub gpu: Option<Gpu>,
    /// Each drive's sectors read and written.
    pub disks: Vec<(String, u64, u64)>,
    pub mounts: Vec<Mount>,
    /// Bytes received and sent by the real network cards.
    pub net: (u64, u64),
    /// The card the way out goes through, and its address.
    pub route: Option<(String, Option<String>)>,
    /// The CPU's energy counters, µJ, each with where it wraps; none when
    /// they can't be read (root only, by default).
    pub rapl: Option<Vec<(u64, u64)>>,
    pub load: [f32; 3],
    pub uptime: f64,
    pub tasks: usize,
    /// Each process by pid; only while it's being looked at.
    pub procs: Option<HashMap<u32, Proc>>,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn number<T: std::str::FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.trim().parse().ok()
}

pub fn info() -> Info {
    let cpuinfo = read("/proc/cpuinfo").unwrap_or_default();
    let field = |name: &str| {
        cpuinfo
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_owned())
    };
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    let cores = field("cpu cores").and_then(|c| c.parse().ok()).unwrap_or(threads);
    let cpu_model = field("model name")
        .unwrap_or_default()
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace(" Processor", "")
        .replace(&format!(" {cores}-Core"), "");
    // The host's, in a Flatpak sandbox.
    let os_release = read("/run/host/os-release").or_else(|| read("/etc/os-release")).unwrap_or_default();
    let os = os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .unwrap_or("Linux")
        .trim_matches('"')
        .to_owned();
    Info {
        cpu_model: cpu_model.split_whitespace().collect::<Vec<_>>().join(" "),
        threads,
        cores,
        disk_models: disk_models(),
        hostname: read("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_owned(),
        os,
        kernel: read("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_owned(),
    }
}

/// The CPU's own sensors, by their driver.
pub const CPU_CHIPS: [&str; 4] = ["k10temp", "coretemp", "zenpower", "cpu_thermal"];

/// What a reading takes.
#[derive(Clone, Copy, Debug)]
pub struct Depth {
    /// Everything, for the popup; else only what the bar shows: the NVMe
    /// drives' temperatures alone took ~18 ms a reading, asking the drives
    /// themselves, maybe out of their power saving.
    pub full: bool,
    /// The processes too.
    pub procs: bool,
    /// The GPU's energy counter, ~10 ms to ask NVIDIA's driver.
    pub gpu_energy: bool,
}

/// Everything now, or less, by `depth`.
pub fn reading(depth: Depth) -> Reading {
    let Depth { full, procs, gpu_energy } = depth;
    let loadavg = read("/proc/loadavg").unwrap_or_default();
    let mut parts = loadavg.split_whitespace();
    let mut load = [0.0; 3];
    for value in &mut load {
        *value = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    }
    let tasks = parts
        .next()
        .and_then(|v| v.split_once('/'))
        .and_then(|(_, all)| all.parse().ok())
        .unwrap_or(0);
    Reading {
        at: Instant::now(),
        cpu: cpu_times(),
        frequency: full.then(frequency).flatten(),
        memory: memory(),
        chips: chips(full),
        gpu: nvidia(full, gpu_energy).or_else(amd_gpu),
        disks: if full { disks() } else { Vec::new() },
        mounts: if full { mounts() } else { Vec::new() },
        net: net(),
        route: full.then(route).flatten(),
        rapl: rapl(),
        load,
        uptime: read("/proc/uptime")
            .and_then(|u| u.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0),
        tasks,
        procs: procs.then(processes),
    }
}

fn cpu_times() -> Vec<CpuTimes> {
    read("/proc/stat")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|line| {
            let values: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
            // user nice system idle iowait irq softirq steal: guest time is
            // in user's already.
            let total: u64 = values.iter().take(8).sum();
            let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
            CpuTimes { busy: total.saturating_sub(idle), total }
        })
        .collect()
}

fn frequency() -> Option<(f32, f32)> {
    let mut sum = 0.0;
    let mut max = 0.0f32;
    let mut count = 0;
    for entry in fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("cpu") || !name[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(khz) = number::<f32>(entry.path().join("cpufreq/scaling_cur_freq")) {
            sum += khz / 1000.0;
            max = max.max(khz / 1000.0);
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f32, max))
}

fn memory() -> Memory {
    let meminfo = read("/proc/meminfo").unwrap_or_default();
    let get = |key: &str| -> u64 {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let total = get("MemTotal");
    let available = get("MemAvailable");
    let cache = (get("Cached") + get("Buffers") + get("SReclaimable")).saturating_sub(get("Shmem"));
    let mut zram = None;
    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with("zram") {
                continue;
            }
            if let Some(stat) = read(entry.path().join("mm_stat")) {
                let v: Vec<u64> = stat.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                if let [original, compressed, ..] = v[..] {
                    let (o, c) = zram.unwrap_or((0, 0));
                    zram = Some((o + original, c + compressed));
                }
            }
        }
    }
    let swap_total = get("SwapTotal");
    Memory {
        total,
        used: total.saturating_sub(available),
        cache: cache.min(available),
        swap_total,
        swap_used: swap_total.saturating_sub(get("SwapFree")),
        zram: zram.filter(|(o, _)| *o > 0),
    }
}

/// The sensors; with `all` false, the CPU's alone.
fn chips(all: bool) -> Vec<Chip> {
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort_by_key(|p| {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        name.trim_start_matches("hwmon").parse::<u32>().unwrap_or(0)
    });
    let mut chips = Vec::new();
    for dir in dirs {
        let kind = read(dir.join("name")).unwrap_or_default().trim().to_owned();
        if !all && !CPU_CHIPS.contains(&kind.as_str()) {
            continue;
        }
        let mut temps = Vec::new();
        let mut fans = Vec::new();
        let Ok(files) = fs::read_dir(&dir) else { continue };
        let mut names: Vec<String> = files.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
        names.sort_by_key(|n| {
            let digits: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
            (n.chars().take(3).collect::<String>(), digits.parse::<u32>().unwrap_or(0))
        });
        for name in names {
            if let Some(index) = name.strip_prefix("temp").and_then(|n| n.strip_suffix("_input")) {
                let Some(milli) = number::<f32>(dir.join(&name)) else { continue };
                let label = read(dir.join(format!("temp{index}_label")))
                    .map(|l| l.trim().to_owned())
                    .unwrap_or_else(|| format!("temp{index}"));
                let critical = number::<f32>(dir.join(format!("temp{index}_crit")))
                    .filter(|c| *c > 0.0 && *c < 200_000.0)
                    .map(|c| c / 1000.0);
                temps.push(Temp { label, celsius: milli / 1000.0, critical });
            } else if let Some(index) = name.strip_prefix("fan").and_then(|n| n.strip_suffix("_input")) {
                if let Some(rpm) = number::<u32>(dir.join(&name)) {
                    let label = read(dir.join(format!("fan{index}_label")))
                        .map(|l| l.trim().to_owned())
                        .unwrap_or_else(|| format!("fan{index}"));
                    fans.push((label, rpm));
                }
            }
        }
        if temps.is_empty() && fans.is_empty() {
            continue;
        }
        // A drive's model, and which block device it is.
        let device = dir.join("device");
        let model = read(device.join("model")).unwrap_or_default().trim().to_owned();
        let disk = fs::read_dir(&device).ok().and_then(|entries| {
            entries.flatten().find_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (name.starts_with("nvme") && name.contains('n') && !name.contains('c'))
                    .then_some(name)
                    .filter(|n| Path::new("/sys/block").join(n).exists())
            })
        });
        chips.push(Chip { kind, model, disk, temps, fans });
    }
    chips
}

fn disks() -> Vec<(String, u64, u64)> {
    read("/proc/diskstats")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let name = *f.get(2)?;
            // Whole drives only: their partitions count again.
            let whole = Path::new("/sys/block").join(name).join("device").exists();
            if !whole {
                return None;
            }
            Some((name.to_owned(), f.get(5)?.parse().ok()?, f.get(9)?.parse().ok()?))
        })
        .collect()
}

/// Each drive's model, as it names itself.
pub fn disk_models() -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir("/sys/block") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let model = read(e.path().join("device/model"))?.trim().to_owned();
            (!model.is_empty()).then(|| (e.file_name().to_string_lossy().into_owned(), model))
        })
        .collect()
}

fn mounts() -> Vec<Mount> {
    let mut mounts: Vec<Mount> = Vec::new();
    for line in read("/proc/self/mounts").unwrap_or_default().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(device), Some(path), Some(kind)) = (f.first(), f.get(1), f.get(2)) else {
            continue;
        };
        if !device.starts_with("/dev/") || device.starts_with("/dev/loop") || *kind == "squashfs" {
            continue;
        }
        let path = path.replace("\\040", " ");
        // One line each: btrfs mounts the same disk many times.
        if let Some(seen) = mounts.iter_mut().find(|m| m.device == *device) {
            if path.len() < seen.path.len() {
                seen.path = path;
            }
            continue;
        }
        let Ok(c_path) = std::ffi::CString::new(path.clone()) else { continue };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
            continue;
        }
        let block = stat.f_frsize as u64;
        let total = stat.f_blocks as u64 * block;
        if total == 0 {
            continue;
        }
        let used = total.saturating_sub(stat.f_bfree as u64 * block);
        mounts.push(Mount { path, device: (*device).to_owned(), total, used });
    }
    mounts.sort_by(|a, b| a.path.cmp(&b.path));
    mounts
}

fn net() -> (u64, u64) {
    let mut total = (0, 0);
    for line in read("/proc/net/dev").unwrap_or_default().lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else { continue };
        // The real cards: bridges, VPNs and containers count what they
        // carry again.
        if !Path::new("/sys/class/net").join(name.trim()).join("device").exists() {
            continue;
        }
        let v: Vec<u64> = values.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        total.0 += v.first().copied().unwrap_or(0);
        total.1 += v.get(8).copied().unwrap_or(0);
    }
    total
}

fn route() -> Option<(String, Option<String>)> {
    let routes = read("/proc/net/route")?;
    let name = routes.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.get(1) == Some(&"00000000")).then(|| f[0].to_owned())
    })?;
    let mut address = None;
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) == 0 {
            let mut item = list;
            while !item.is_null() {
                let ifa = &*item;
                if !ifa.ifa_addr.is_null()
                    && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_INET
                    && CStr::from_ptr(ifa.ifa_name).to_string_lossy() == name
                {
                    let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    address = Some(std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)).to_string());
                }
                item = ifa.ifa_next;
            }
            libc::freeifaddrs(list);
        }
    }
    Some((name, address))
}

fn rapl() -> Option<Vec<(u64, u64)>> {
    let mut zones = Vec::new();
    for entry in fs::read_dir("/sys/class/powercap").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // The packages, not their parts, which they count already.
        if !name.starts_with("intel-rapl:") || name.matches(':').count() != 1 {
            continue;
        }
        let energy = number::<u64>(entry.path().join("energy_uj"))?;
        let range = number::<u64>(entry.path().join("max_energy_range_uj")).unwrap_or(u64::MAX);
        zones.push((energy, range));
    }
    (!zones.is_empty()).then_some(zones)
}

/// What each pid runs, kept: reading where its program is each second for
/// every process cost more than the rest.
static NAMES: LazyLock<Mutex<HashMap<u32, (u64, String)>>> = LazyLock::new(Default::default);

fn processes() -> HashMap<u32, Proc> {
    let uid = unsafe { libc::getuid() };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let mut procs = HashMap::new();
    let mut names = NAMES.lock().unwrap();
    let Ok(entries) = fs::read_dir("/proc") else { return procs };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let dir = entry.path();
        let Some(stat) = read(dir.join("stat")) else { continue };
        let Some((head, rest)) = stat.rsplit_once(')') else { continue };
        let comm = head.split_once('(').map(|(_, c)| c).unwrap_or_default();
        let f: Vec<&str> = rest.split_whitespace().collect();
        // After the name: state is 0, utime 11, stime 12, starttime 19.
        let ticks = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
            + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let start: u64 = f.get(19).and_then(|v| v.parse().ok()).unwrap_or(0);
        let Some(statm) = read(dir.join("statm")) else { continue };
        let m: Vec<u64> = statm.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        let (resident, shared) = (m.get(1).copied().unwrap_or(0), m.get(2).copied().unwrap_or(0));
        // Kernel threads have no memory of their own.
        if resident == 0 {
            continue;
        }
        let name = match names.get(&pid) {
            Some((started, name)) if *started == start => name.clone(),
            _ => {
                let name = fs::read_link(dir.join("exe"))
                    .ok()
                    .and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()))
                    .map(|n| n.trim_end_matches(" (deleted)").to_owned())
                    .unwrap_or_else(|| comm.to_owned());
                names.insert(pid, (start, name.clone()));
                name
            }
        };
        use std::os::unix::fs::MetadataExt;
        let mine = entry.metadata().is_ok_and(|m| m.uid() == uid);
        procs.insert(pid, Proc { name, ticks, memory: resident.saturating_sub(shared) * page, mine });
    }
    names.retain(|pid, _| procs.contains_key(pid));
    procs
}

/// Ends a process, as `kill` does; false when it isn't ours to end.
pub fn terminate(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
}

// NVIDIA's cards, through the driver's own library, loaded when there: a
// call each, where running nvidia-smi took a program each second.

type Device = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

struct Nvml {
    device: Device,
    name: String,
    utilization: unsafe extern "C" fn(Device, *mut Utilization) -> i32,
    memory: unsafe extern "C" fn(Device, *mut NvmlMemory) -> i32,
    temperature: unsafe extern "C" fn(Device, u32, *mut u32) -> i32,
    power: unsafe extern "C" fn(Device, *mut u32) -> i32,
    limit: unsafe extern "C" fn(Device, *mut u32) -> i32,
    clock: unsafe extern "C" fn(Device, u32, *mut u32) -> i32,
    fan: unsafe extern "C" fn(Device, *mut u32) -> i32,
    energy: unsafe extern "C" fn(Device, *mut u64) -> i32,
}

// The library is safe to call from any thread.
unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

static NVML: LazyLock<Option<Nvml>> = LazyLock::new(|| unsafe { load_nvml() });

unsafe fn load_nvml() -> Option<Nvml> {
    unsafe {
        if !Path::new("/proc/driver/nvidia").exists() {
            return None;
        }
        let lib = libc::dlopen(c"libnvidia-ml.so.1".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            return None;
        }
        macro_rules! symbol {
            ($name:literal) => {{
                let pointer = libc::dlsym(lib, $name.as_ptr());
                if pointer.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(pointer)
            }};
        }
        let init: unsafe extern "C" fn() -> i32 = symbol!(c"nvmlInit_v2");
        if init() != 0 {
            return None;
        }
        let by_index: unsafe extern "C" fn(u32, *mut Device) -> i32 =
            symbol!(c"nvmlDeviceGetHandleByIndex_v2");
        let mut device: Device = std::ptr::null_mut();
        if by_index(0, &mut device) != 0 {
            return None;
        }
        let get_name: unsafe extern "C" fn(Device, *mut c_char, u32) -> i32 = symbol!(c"nvmlDeviceGetName");
        let mut buffer = [0 as c_char; 96];
        let name = if get_name(device, buffer.as_mut_ptr(), buffer.len() as u32) == 0 {
            CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned()
        } else {
            "NVIDIA".to_owned()
        };
        Some(Nvml {
            device,
            name: name.replace("NVIDIA ", ""),
            utilization: symbol!(c"nvmlDeviceGetUtilizationRates"),
            memory: symbol!(c"nvmlDeviceGetMemoryInfo"),
            temperature: symbol!(c"nvmlDeviceGetTemperature"),
            power: symbol!(c"nvmlDeviceGetPowerUsage"),
            limit: symbol!(c"nvmlDeviceGetEnforcedPowerLimit"),
            clock: symbol!(c"nvmlDeviceGetClockInfo"),
            fan: symbol!(c"nvmlDeviceGetFanSpeed"),
            energy: symbol!(c"nvmlDeviceGetTotalEnergyConsumption"),
        })
    }
}

/// Use, temperature and watts take microseconds; the fan and the energy
/// counter, milliseconds: those only when `full` and `energy`.
fn nvidia(full: bool, energy: bool) -> Option<Gpu> {
    let nvml = NVML.as_ref()?;
    let d = nvml.device;
    unsafe {
        let mut utilization = Utilization::default();
        let usage = ((nvml.utilization)(d, &mut utilization) == 0).then_some(utilization.gpu as f32);
        let mut memory = NvmlMemory::default();
        if full {
            (nvml.memory)(d, &mut memory);
        }
        let value = |f: unsafe extern "C" fn(Device, *mut u32) -> i32| {
            let mut v = 0;
            (f(d, &mut v) == 0).then_some(v)
        };
        let mut temp = 0;
        let temp = ((nvml.temperature)(d, 0, &mut temp) == 0).then_some(temp as f32);
        let mut clock = 0;
        let clock = ((nvml.clock)(d, 0, &mut clock) == 0).then_some(clock);
        let mut counter = 0;
        let energy_mj = (energy && (nvml.energy)(d, &mut counter) == 0).then_some(counter);
        Some(Gpu {
            name: nvml.name.clone(),
            usage: usage.unwrap_or(0.0),
            vram_used: memory.used,
            vram_total: memory.total,
            temp,
            watts: value(nvml.power).map(|mw| mw as f32 / 1000.0),
            watts_limit: value(nvml.limit).map(|mw| mw as f32 / 1000.0),
            clock,
            fan: if full { value(nvml.fan) } else { None },
            energy_mj,
        })
    }
}

/// AMD's cards say it all in sysfs.
fn amd_gpu() -> Option<Gpu> {
    for entry in fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        if read(device.join("vendor")).is_none_or(|v| v.trim() != "0x1002") {
            continue;
        }
        let Some(usage) = number::<f32>(device.join("gpu_busy_percent")) else { continue };
        let hwmon = fs::read_dir(device.join("hwmon")).ok().and_then(|mut d| d.next()?.ok()).map(|e| e.path());
        let from_hwmon = |file: &str| hwmon.as_ref().and_then(|h| number::<f64>(h.join(file)));
        return Some(Gpu {
            name: read(device.join("product_name")).map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
                .unwrap_or_else(|| "Radeon".to_owned()),
            usage,
            vram_used: number(device.join("mem_info_vram_used")).unwrap_or(0),
            vram_total: number(device.join("mem_info_vram_total")).unwrap_or(0),
            temp: from_hwmon("temp1_input").map(|t| (t / 1000.0) as f32),
            watts: from_hwmon("power1_average").or_else(|| from_hwmon("power1_input")).map(|w| (w / 1e6) as f32),
            watts_limit: from_hwmon("power1_cap").map(|w| (w / 1e6) as f32),
            clock: from_hwmon("freq1_input").map(|hz| (hz / 1e6) as u32),
            fan: None,
            energy_mj: None,
        });
    }
    None
}
