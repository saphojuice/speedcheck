// SAPHOJUICE probe v0.1
// Measures the physical constants that determine local LLM speed on this machine:
//   1. memory bandwidth per tier (cache -> DRAM), multi- and single-threaded
//   2. compute throughput (f32 GEMM, int8 dot product)
//   3. sustained ceilings under load (thermal / power throttling)
//   4. storage sequential read/write
//   5. hardware inventory (CPU, ISA features, RAM modules, GPU, power state)
// Then derives predicted decode tok/s for a reference model table using
//   tok/s = k * bandwidth / active_bytes_per_token
// Zero external crates. Builds on Windows (x64 / ARM64), macOS, Linux.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const VERSION: &str = "0.1.1";
// Uncalibrated efficiency factor, replaced by a measured k when a generation run happens.
// 0.8 matches what the browser test and the share pages assume, so the same machine gets
// the same answer from the web and the CLI. It is also closer to what was actually measured
// on the reference machine (k = 0.83) than the 0.6 this used to assume.
const DEFAULT_K: f64 = 0.8;
const OS_RESERVE_GB: f64 = 4.0;
const CTX_TOKENS: f64 = 8192.0;
const CALIBRATION_SECONDS: f64 = 30.0;
const MANIFEST_JSON: &str = include_str!("../manifest.json");

// ---------------------------------------------------------------- CLI

struct Opts {
    bw_seconds: u64,
    gemm_seconds: u64,
    quick: bool,
    out: String,
    skip_storage: bool,
    offline: bool,
    no_share: bool,
    show_payload: bool,
}

fn parse_opts() -> Result<Opts, String> {
    let mut o = Opts {
        bw_seconds: 60,
        gemm_seconds: 20,
        quick: false,
        out: String::new(),
        skip_storage: false,
        offline: false,
        no_share: false,
        show_payload: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bw-seconds" => {
                i += 1;
                o.bw_seconds = args.get(i).and_then(|s| s.parse().ok()).ok_or("bad --bw-seconds")?;
            }
            "--gemm-seconds" => {
                i += 1;
                o.gemm_seconds = args.get(i).and_then(|s| s.parse().ok()).ok_or("bad --gemm-seconds")?;
            }
            "--out" => {
                i += 1;
                o.out = args.get(i).cloned().ok_or("bad --out")?;
            }
            "--quick" => {
                o.quick = true;
                o.bw_seconds = 10;
                o.gemm_seconds = 5;
            }
            "--skip-storage" => o.skip_storage = true,
            "--offline" => o.offline = true,
            "--no-share" => o.no_share = true,
            "--show-payload" => o.show_payload = true,
            "-h" | "--help" => {
                println!("sj-probe [--quick] [--bw-seconds N] [--gemm-seconds N] [--skip-storage] [--offline]");
                println!("         [--no-share] [--show-payload] [--out FILE]");
                println!("  --offline       skip the model download and generation run; print physical measurements only");
                println!("  --no-share      never ask and never send; measure and print only");
                println!("  --show-payload  print the exact JSON that would be sent, before asking");
                println!("  --out FILE      also write the full receipt JSON here (nothing is written otherwise)");
                println!("sj-probe k --tokps X --model-gb Y --bw Z    compute efficiency factor k from a measured run");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg {}", other)),
        }
        i += 1;
    }
    Ok(o)
}

// ---------------------------------------------------------------- tiny JSON writer

fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{:.4}", v)
    } else {
        "null".to_string()
    }
}

fn jopt(v: Option<f64>) -> String {
    match v {
        Some(x) => jnum(x),
        None => "null".to_string(),
    }
}

// ---------------------------------------------------------------- shell helpers

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(target_os = "windows")]
fn ps(script: &str) -> Option<String> {
    run("powershell", &["-NoProfile", "-NonInteractive", "-Command", script])
}

#[cfg(not(target_os = "windows"))]
fn ps(_script: &str) -> Option<String> {
    None
}

fn read_file(path: &str) -> Option<String> {
    let mut s = String::new();
    File::open(path).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

// ---------------------------------------------------------------- inventory

struct Inventory {
    os: String,
    arch: String,
    hostname_hash: String,
    cpu_name: String,
    logical_cores: usize,
    isa: Vec<String>,
    ram_total_gb: Option<f64>,
    ram_available_gb: Option<f64>,
    ram_modules_json: String, // raw JSON from WMI or "null"
    gpu_json: String,
    power_plan: String,
    battery_json: String,
    raw_notes: Vec<String>,
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn isa_features() -> Vec<String> {
    let mut v = Vec::new();
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") { v.push("neon".into()); }
        if std::arch::is_aarch64_feature_detected!("dotprod") { v.push("dotprod".into()); }
        if std::arch::is_aarch64_feature_detected!("i8mm") { v.push("i8mm".into()); }
        if std::arch::is_aarch64_feature_detected!("fp16") { v.push("fp16".into()); }
        if std::arch::is_aarch64_feature_detected!("bf16") { v.push("bf16".into()); }
        if std::arch::is_aarch64_feature_detected!("sve") { v.push("sve".into()); }
        if std::arch::is_aarch64_feature_detected!("sve2") { v.push("sve2".into()); }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::arch::is_x86_feature_detected!("avx2") { v.push("avx2".into()); }
        if std::arch::is_x86_feature_detected!("fma") { v.push("fma".into()); }
        if std::arch::is_x86_feature_detected!("f16c") { v.push("f16c".into()); }
        if std::arch::is_x86_feature_detected!("avx512f") { v.push("avx512f".into()); }
        if std::arch::is_x86_feature_detected!("avx512vnni") { v.push("avx512vnni".into()); }
        if std::arch::is_x86_feature_detected!("avxvnni") { v.push("avxvnni".into()); }
    }
    v
}

fn parse_first_number(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    t.parse().ok()
}

fn inventory() -> Inventory {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let logical_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mut notes = Vec::new();

    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| run("hostname", &[]).unwrap_or_default());
    let hostname_hash = format!("{:016x}", fnv1a(&host));

    let mut cpu_name = String::from("unknown");
    let mut ram_total_gb = None;
    let mut ram_available_gb = None;
    let mut ram_modules_json = "null".to_string();
    let mut gpu_json = "null".to_string();
    let mut power_plan = "unknown".to_string();
    let mut battery_json = "null".to_string();

    if os == "windows" {
        if let Some(s) = ps("(Get-CimInstance Win32_Processor | Select-Object -First 1).Name") {
            cpu_name = s;
        }
        if let Some(s) = ps("(Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize") {
            ram_total_gb = parse_first_number(&s).map(|kb| kb / 1024.0 / 1024.0);
        }
        if let Some(s) = ps("(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory") {
            ram_available_gb = parse_first_number(&s).map(|kb| kb / 1024.0 / 1024.0);
        }
        if let Some(s) = ps("Get-CimInstance Win32_PhysicalMemory | Select-Object Capacity,Speed,ConfiguredClockSpeed,SMBIOSMemoryType,FormFactor,DataWidth,DeviceLocator | ConvertTo-Json -Compress") {
            ram_modules_json = if s.starts_with('[') { s } else { format!("[{}]", s) };
        }
        if let Some(s) = ps("Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion,VideoProcessor | ConvertTo-Json -Compress") {
            gpu_json = if s.starts_with('[') { s } else { format!("[{}]", s) };
        }
        if let Some(s) = run("powercfg", &["/getactivescheme"]) {
            power_plan = s;
        }
        if let Some(s) = ps("Get-CimInstance Win32_Battery | Select-Object BatteryStatus,EstimatedChargeRemaining | ConvertTo-Json -Compress") {
            battery_json = if s.starts_with('[') { s } else { format!("[{}]", s) };
        } else {
            battery_json = "null".to_string();
        }
        // This tested the legacy plan, which reads "Balanced" even when the Windows 11
        // power mode slider is set to Best performance, so it fired on machines already
        // configured correctly. Test the real mode instead.
        let m = windows_power_mode(None, &power_plan).to_lowercase();
        if m.contains("efficiency") || m.contains("power saver") {
            notes.push("Windows power mode is not Best performance; ARM laptops in particular clock down. Rerun on Best performance, plugged in.".to_string());
        }
    } else if os == "linux" {
        if let Some(s) = read_file("/proc/cpuinfo") {
            for line in s.lines() {
                if line.starts_with("model name") || line.starts_with("Model") {
                    if let Some(v) = line.split(':').nth(1) {
                        cpu_name = v.trim().to_string();
                        break;
                    }
                }
            }
        }
        if let Some(s) = read_file("/proc/meminfo") {
            for line in s.lines() {
                if line.starts_with("MemTotal:") {
                    ram_total_gb = parse_first_number(line).map(|kb| kb / 1024.0 / 1024.0);
                }
                if line.starts_with("MemAvailable:") {
                    ram_available_gb = parse_first_number(line).map(|kb| kb / 1024.0 / 1024.0);
                }
            }
        }
        if let Some(s) = run("sh", &["-c", "lspci 2>/dev/null | grep -i -E 'vga|3d|display'"]) {
            gpu_json = format!("[{}]", jstr(&s));
        }
    } else if os == "macos" {
        if let Some(s) = run("sysctl", &["-n", "machdep.cpu.brand_string"]) {
            cpu_name = s;
        }
        if let Some(s) = run("sysctl", &["-n", "hw.memsize"]) {
            ram_total_gb = parse_first_number(&s).map(|b| b / 1024.0 / 1024.0 / 1024.0);
        }
        if let Some(s) = run("sh", &["-c", "system_profiler SPDisplaysDataType 2>/dev/null | grep -E 'Chipset Model|VRAM' | head -4"]) {
            gpu_json = format!("[{}]", jstr(&s));
        }
        if let Some(s) = run("pmset", &["-g", "batt"]) {
            battery_json = jstr(&s);
        }
    }

    Inventory {
        os,
        arch,
        hostname_hash,
        cpu_name,
        logical_cores,
        isa: isa_features(),
        ram_total_gb,
        ram_available_gb,
        ram_modules_json,
        gpu_json,
        power_plan,
        battery_json,
        raw_notes: notes,
    }
}

// ---------------------------------------------------------------- bandwidth (STREAM triad, per-thread private buffers)

struct BwPoint {
    bytes_per_thread: usize,
    threads: usize,
    gbps: f64,
}

// Each thread owns its own a,b,c buffers and runs the triad for `budget` seconds.
// Aggregate bandwidth = sum(bytes moved) / max(thread elapsed). This measures the
// cache tier that `bytes_per_thread * 3` fits into, without cross-thread sharing.
fn triad_bw(bytes_per_thread: usize, threads: usize, budget: Duration) -> f64 {
    let n = (bytes_per_thread / 8).max(1024);
    let results: Vec<(f64, f64)> = std::thread::scope(|sc| {
        let mut handles = Vec::with_capacity(threads);
        for t in 0..threads {
            handles.push(sc.spawn(move || {
                let s: f64 = 1.000001 + (t as f64) * 1e-9;
                let mut a = vec![0.0f64; n];
                let b: Vec<f64> = (0..n).map(|i| (i % 97) as f64).collect();
                let c: Vec<f64> = (0..n).map(|i| (i % 89) as f64 * 0.5).collect();
                // warm-up
                for ((ai, bi), ci) in a.iter_mut().zip(b.iter()).zip(c.iter()) {
                    *ai = *bi + s * *ci;
                }
                let start = Instant::now();
                let mut reps: u64 = 0;
                while start.elapsed() < budget {
                    for ((ai, bi), ci) in a.iter_mut().zip(b.iter()).zip(c.iter()) {
                        *ai = *bi + s * *ci;
                    }
                    reps += 1;
                    std::hint::black_box(&a);
                }
                let secs = start.elapsed().as_secs_f64();
                let bytes = (reps as f64) * 3.0 * 8.0 * (n as f64);
                (bytes, secs)
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let total_bytes: f64 = results.iter().map(|r| r.0).sum();
    let max_secs = results.iter().map(|r| r.1).fold(0.0, f64::max);
    if max_secs > 0.0 {
        total_bytes / max_secs / 1e9
    } else {
        0.0
    }
}

// Bandwidth at 1, half and all cores, at a working set far larger than any cache, so the best
// thread count is measured rather than assumed. More threads stop helping once the workload is
// waiting on memory, and on some chips they make it worse.
fn thread_sweep(cores: usize, quick: bool) -> Vec<(usize, f64)> {
    let mut counts = vec![1usize];
    if cores >= 4 { counts.push(cores / 2); }
    if cores > 1 { counts.push(cores); }
    counts.dedup();
    let budget = Duration::from_millis(if quick { 400 } else { 900 });
    let per_thread = 64 * 1024 * 1024;
    // Best of several passes, first one discarded.
    //
    // A single pass is not reproducible. Measured on one machine, five consecutive runs of the
    // old single-pass code gave 22.2, 33.4, 33.8, 34.6 and 34.6 GB/s single-threaded: the first
    // pass after an idle period runs before the CPU has boosted and while the pages are still
    // cold, so it reads about a third low. Taking the best of several passes removes that, and
    // the best is the right statistic anyway: this is a ceiling, and interference only ever
    // pushes it down.
    let passes = if quick { 2 } else { 3 };
    counts.into_iter().map(|c| {
        let bytes = per_thread / c.max(1);
        let _warmup = triad_bw(bytes, c, Duration::from_millis(120));   // discarded
        let mut best = 0.0f64;
        for _ in 0..passes {
            let g = triad_bw(bytes, c, budget);
            if g > best { best = g; }
        }
        (c, best)
    }).collect()
}

fn bandwidth_sweep(cores: usize, ram_available_gb: Option<f64>, quick: bool) -> (Vec<BwPoint>, Vec<BwPoint>) {
    // sizes are total working set across all threads (a+b+c), chosen to straddle L2, L3, DRAM
    let mut sizes_total: Vec<usize> = vec![
        192 * 1024,
        1536 * 1024,
        6 * 1024 * 1024,
        24 * 1024 * 1024,
        96 * 1024 * 1024,
        384 * 1024 * 1024,
        1536 * 1024 * 1024,
    ];
    let cap_bytes = ((ram_available_gb.unwrap_or(8.0) * 0.25) * 1e9) as usize;
    sizes_total.retain(|s| *s <= cap_bytes.max(64 * 1024 * 1024));
    let budget = if quick { Duration::from_millis(250) } else { Duration::from_millis(600) };

    let mut multi = Vec::new();
    let mut single = Vec::new();
    for &total in &sizes_total {
        let per_thread = total / 3 / cores;
        let g = triad_bw(per_thread, cores, budget);
        eprintln!("  bw  {:>8.1} MB working set  x{} threads  {:>8.2} GB/s", total as f64 / 1e6, cores, g);
        multi.push(BwPoint { bytes_per_thread: per_thread, threads: cores, gbps: g });
    }
    // single-thread on a subset (small, mid, largest)
    let picks: Vec<usize> = if sizes_total.len() >= 3 {
        vec![sizes_total[0], sizes_total[sizes_total.len() / 2], *sizes_total.last().unwrap()]
    } else {
        sizes_total.clone()
    };
    for &total in &picks {
        let per_thread = total / 3;
        let g = triad_bw(per_thread, 1, budget);
        eprintln!("  bw  {:>8.1} MB working set  x1 thread   {:>8.2} GB/s", total as f64 / 1e6, g);
        single.push(BwPoint { bytes_per_thread: per_thread, threads: 1, gbps: g });
    }
    (multi, single)
}

// ---------------------------------------------------------------- compute

// f32 GEMM C = A*B, n x n, i-k-j ordering (row-major, inner loop streams contiguous), rows split across threads.
fn gemm_gflops(n: usize, threads: usize, budget: Duration) -> f64 {
    let a: Vec<f32> = (0..n * n).map(|i| ((i % 13) as f32) * 0.01).collect();
    let b: Vec<f32> = (0..n * n).map(|i| ((i % 7) as f32) * 0.02).collect();
    let mut c = vec![0.0f32; n * n];
    let rows_per = (n + threads - 1) / threads;
    let mut best = 0.0f64;
    let start_all = Instant::now();
    while start_all.elapsed() < budget {
        let t0 = Instant::now();
        std::thread::scope(|sc| {
            for (ti, crows) in c.chunks_mut(rows_per * n).enumerate() {
                let a = &a;
                let b = &b;
                sc.spawn(move || {
                    let r0 = ti * rows_per;
                    let nrows = crows.len() / n;
                    for r in 0..nrows {
                        let ar = &a[(r0 + r) * n..(r0 + r + 1) * n];
                        let cr = &mut crows[r * n..(r + 1) * n];
                        for x in cr.iter_mut() {
                            *x = 0.0;
                        }
                        for k in 0..n {
                            let aik = ar[k];
                            let brow = &b[k * n..(k + 1) * n];
                            for (cj, bj) in cr.iter_mut().zip(brow.iter()) {
                                *cj += aik * *bj;
                            }
                        }
                    }
                });
            }
        });
        let secs = t0.elapsed().as_secs_f64();
        std::hint::black_box(&c);
        let gf = 2.0 * (n as f64).powi(3) / secs / 1e9;
        if gf > best {
            best = gf;
        }
    }
    best
}

// int8 dot products with i32 accumulation; auto-vectorizes to dotprod / VNNI when target-cpu allows.
fn int8_gops(threads: usize, budget: Duration) -> f64 {
    let len = 8192usize;
    let results: Vec<(f64, f64)> = std::thread::scope(|sc| {
        let mut hs = Vec::new();
        for t in 0..threads {
            hs.push(sc.spawn(move || {
                let x: Vec<i8> = (0..len).map(|i| ((i * 7 + t) % 127) as i8 - 63).collect();
                let y: Vec<i8> = (0..len).map(|i| ((i * 11 + t) % 127) as i8 - 63).collect();
                let start = Instant::now();
                let mut reps: u64 = 0;
                let mut sink: i64 = 0;
                while start.elapsed() < budget {
                    let mut acc: i32 = 0;
                    for (xi, yi) in x.iter().zip(y.iter()) {
                        acc = acc.wrapping_add((*xi as i32) * (*yi as i32));
                    }
                    sink = sink.wrapping_add(acc as i64);
                    reps += 1;
                }
                std::hint::black_box(sink);
                let secs = start.elapsed().as_secs_f64();
                ((reps as f64) * 2.0 * (len as f64), secs)
            }));
        }
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let ops: f64 = results.iter().map(|r| r.0).sum();
    let max_secs = results.iter().map(|r| r.1).fold(0.0, f64::max);
    if max_secs > 0.0 {
        ops / max_secs / 1e9
    } else {
        0.0
    }
}

// ---------------------------------------------------------------- sustained

// Samples a metric every `window` seconds for `total` seconds; returns the series.
fn sustained<F: Fn(Duration) -> f64>(total_secs: u64, window_secs: u64, f: F, label: &str) -> Vec<f64> {
    let mut series = Vec::new();
    let windows = (total_secs / window_secs).max(1);
    for w in 0..windows {
        let v = f(Duration::from_secs(window_secs));
        eprintln!("  sustained {:<6} t={:>3}s  {:>8.2}", label, (w + 1) * window_secs, v);
        series.push(v);
    }
    series
}

// ---------------------------------------------------------------- storage

fn storage_test() -> Option<(f64, f64)> {
    let dir = std::env::temp_dir();
    let path = dir.join("sj_probe_storage_test.bin");
    let block = 4usize * 1024 * 1024;
    let blocks = 128usize; // 512 MB
    let mut buf = vec![0u8; block];
    let mut x: u64 = 0x9E3779B97F4A7C15;
    for b in buf.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x & 0xff) as u8;
    }
    let t0 = Instant::now();
    {
        let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(&path).ok()?;
        for _ in 0..blocks {
            f.write_all(&buf).ok()?;
        }
        f.sync_all().ok()?;
    }
    let wsecs = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    {
        let mut f = File::open(&path).ok()?;
        let mut rb = vec![0u8; block];
        let mut sink: u64 = 0;
        loop {
            let n = f.read(&mut rb).ok()?;
            if n == 0 {
                break;
            }
            sink = sink.wrapping_add(rb[0] as u64).wrapping_add(rb[n - 1] as u64);
        }
        std::hint::black_box(sink);
    }
    let rsecs = t1.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&path);
    let bytes = (block * blocks) as f64;
    Some((bytes / wsecs / 1e9, bytes / rsecs / 1e9))
}

// ---------------------------------------------------------------- SHA-256 (pure Rust, no crates)

struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                Self::compress(&mut self.h, &block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            Self::compress(&mut self.h, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let bit_len: u64 = self.total_len * 8;
        let mut idx = self.buf_len;
        self.buf[idx] = 0x80;
        idx += 1;
        if idx <= 56 {
            for b in &mut self.buf[idx..56] {
                *b = 0;
            }
        } else {
            for b in &mut self.buf[idx..64] {
                *b = 0;
            }
            let block = self.buf;
            Self::compress(&mut self.h, &block);
            self.buf = [0u8; 64];
        }
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        Self::compress(&mut self.h, &block);
        let mut out = String::with_capacity(64);
        for word in self.h.iter() {
            out.push_str(&format!("{:08x}", word));
        }
        out
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize_hex()
}

// ---------------------------------------------------------------- tiny JSON reader (flat key lookup; enough for our own manifest and llama-bench's output)

fn json_str(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let pos = json.find(&pat)?;
    let after = json[pos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

fn json_num(json: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{}\"", key);
    let pos = json.find(&pat)?;
    let after = json[pos + pat.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let end = after.find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace()).unwrap_or(after.len());
    after[..end].trim().parse::<f64>().ok()
}

// ---------------------------------------------------------------- target triple (for engines/<target>/ lookup)

fn target_triple() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "windows", target_env = "msvc"))]
    {
        "aarch64-pc-windows-msvc"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"),
        all(target_arch = "aarch64", target_os = "windows", target_env = "msvc"),
        all(target_arch = "x86_64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
    )))]
    {
        "unknown"
    }
}

// looks next to the running binary first (release layout: exe + engines/<target>/ shipped side by side),
// then in the dev checkout layout (speedcheck/target/release/sj-probe.exe with speedcheck/engines/<target>/).
fn find_engine(target: &str) -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") { "llama-bench.exe" } else { "llama-bench" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("engines").join(target).join(exe_name));
            candidates.push(dir.join("..").join("engines").join(target).join(exe_name));
            candidates.push(dir.join("..").join("..").join("engines").join(target).join(exe_name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("engines").join(target).join(exe_name));
        candidates.push(cwd.join("speedcheck").join("engines").join(target).join(exe_name));
    }
    candidates.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------------- model calibration: download, verify, generate

struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Calibration {
    model_name: String,
    file_gb: f64,
    engine_path: String,
    threads: usize,
    n_gen: u64,
    measured_tps: f64,
    ceiling_tps: f64,
    k_measured: f64,
}

fn download_and_verify(url: &str, dest: &Path, expected_sha256: &str, expected_size: u64) -> Result<(), String> {
    eprintln!("  downloading {}", url);
    let agent = http_agent(Duration::from_secs(1800));
    let mut resp = agent.get(url).call().map_err(|e| format!("request failed: {e}"))?;
    let mut reader = resp.body_mut().as_reader();
    let mut file = File::create(dest).map_err(|e| format!("create temp file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20]; // heap-allocated: too large for the default thread stack
    let mut total: u64 = 0;
    let mut last_report = Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
        total += n as u64;
        if last_report.elapsed() > Duration::from_secs(2) {
            eprintln!("    {:.0} / {:.0} MB", total as f64 / 1e6, expected_size as f64 / 1e6);
            last_report = Instant::now();
        }
    }
    file.sync_all().ok();
    drop(file);
    if total != expected_size {
        return Err(format!("size mismatch: got {} bytes, manifest says {}", total, expected_size));
    }
    let got = hasher.finalize_hex();
    if got != expected_sha256 {
        return Err(format!("sha256 mismatch: got {}, manifest says {}", got, expected_sha256));
    }
    Ok(())
}

fn parse_llama_bench_tps(json: &str) -> Option<f64> {
    let bytes = json.as_bytes();
    let mut depth = 0i32;
    let mut start = None;
    let mut best: Option<f64> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let obj = &json[s..=i];
                        if json_num(obj, "n_gen").unwrap_or(0.0) > 0.0 {
                            if let Some(ts) = json_num(obj, "avg_ts") {
                                best = Some(ts);
                            }
                        }
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    best
}

fn llama_bench_tps(engine: &Path, model: &Path, threads: usize, n_gen: u64) -> Result<f64, String> {
    let out = Command::new(engine)
        .arg("-m")
        .arg(model)
        .arg("-p")
        .arg("0")
        .arg("-n")
        .arg(n_gen.to_string())
        .arg("-t")
        .arg(threads.to_string())
        .arg("-r")
        .arg("1")
        .arg("-o")
        .arg("json")
        .output()
        .map_err(|e| format!("failed to run engine: {e}"))?;
    if !out.status.success() {
        return Err(format!("engine exited with {}: {}", out.status, String::from_utf8_lossy(&out.stderr)));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_llama_bench_tps(&stdout).ok_or_else(|| "could not parse engine output (unexpected llama-bench JSON shape)".to_string())
}

// Two passes: a short probe run sizes the token count so the real run takes roughly CALIBRATION_SECONDS.
fn measure_generation(engine: &Path, model: &Path, threads: usize) -> Result<(f64, u64), String> {
    eprintln!("  engine: {}", engine.display());
    eprintln!("  sizing run...");
    let probe_tps = llama_bench_tps(engine, model, threads, 32)?;
    if probe_tps <= 0.0 {
        return Err("engine reported 0 tok/s on the sizing run".to_string());
    }
    let n_gen = ((probe_tps * CALIBRATION_SECONDS).round() as u64).clamp(128, 8192);
    eprintln!("  measuring ~{:.0}s generation, {} tokens, {} threads...", CALIBRATION_SECONDS, n_gen, threads);
    let tps = llama_bench_tps(engine, model, threads, n_gen)?;
    Ok((tps, n_gen))
}

fn run_calibration(inv: &Inventory, bw_gbps: f64) -> Result<Calibration, String> {
    let target = target_triple();
    let engine = find_engine(target).ok_or_else(|| {
        let ext = if cfg!(windows) { ".exe" } else { "" };
        format!(
            "no prebuilt llama.cpp engine for this machine's ISA ({target}). Place a llama-bench build at speedcheck/engines/{target}/llama-bench{ext} to enable a live generation run; showing the fit list above without a run."
        )
    })?;

    let name = json_str(MANIFEST_JSON, "name").ok_or("manifest.json missing model.name")?;
    let file = json_str(MANIFEST_JSON, "file").ok_or("manifest.json missing model.file")?;
    let url = json_str(MANIFEST_JSON, "url").ok_or("manifest.json missing model.url")?;
    let sha256 = json_str(MANIFEST_JSON, "sha256").ok_or("manifest.json missing model.sha256")?.to_lowercase();
    let size_bytes = json_num(MANIFEST_JSON, "size_bytes").ok_or("manifest.json missing model.size_bytes")? as u64;

    let work_dir = std::env::temp_dir().join(format!("sj-speedcheck-{}", std::process::id()));
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("could not create temp dir: {e}"))?;
    let _cleanup = TempDirGuard(work_dir.clone()); // removes work_dir (and the downloaded model) when this function returns
    let model_path = work_dir.join(&file);

    let file_gb = size_bytes as f64 / 1e9;
    eprintln!("  model: {} ({:.2} GB) -> {}", name, file_gb, work_dir.display());
    download_and_verify(&url, &model_path, &sha256, size_bytes)?;
    eprintln!("  sha256 verified against manifest.json");

    let threads = inv.logical_cores;
    let (measured_tps, n_gen) = measure_generation(&engine, &model_path, threads)?;
    let ceiling_tps = if file_gb > 0.0 { bw_gbps / file_gb } else { 0.0 };
    let k_measured = if ceiling_tps > 0.0 { measured_tps / ceiling_tps } else { 0.0 };

    println!();
    println!(
        "Calibration: {} measured {:.1} tok/s (ceiling {:.1} tok/s = {:.1} GB/s / {:.2} GB), k = {:.3}",
        name, measured_tps, ceiling_tps, bw_gbps, file_gb, k_measured
    );
    if k_measured > 1.05 {
        println!("  k > 1: model bytes or bandwidth is wrong, or a GPU did the work instead of the CPU path measured above.");
    } else if k_measured < 0.3 {
        println!("  k < 0.3: something is misconfigured (build, thread count, power plan, or RAM spill).");
    }

    Ok(Calibration {
        model_name: name,
        file_gb,
        engine_path: engine.display().to_string(),
        threads,
        n_gen,
        measured_tps,
        ceiling_tps,
        k_measured,
    })
}

// best-effort one-line GPU descriptor pulled out of the raw inventory JSON blob, shaped to match
// what the worker expects ({description}), independent of the raw machine.gpu field kept in the receipt.
fn gpu_description(inv: &Inventory) -> Option<String> {
    for key in ["Name", "VideoProcessor", "description"] {
        if let Some(v) = json_str(&inv.gpu_json, key) {
            if !v.trim().is_empty() {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// One place that knows which TLS provider this build actually has. ureq decides at runtime and
/// defaults to Rustls, so a native-tls-only build panics on the first https request unless told.
fn http_agent(timeout: Duration) -> ureq::Agent {
    let builder = ureq::Agent::config_builder().timeout_global(Some(timeout));
    #[cfg(windows)]
    let builder = builder.tls_config(
        ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            // building a TlsConfig from scratch drops the default root store. SChannel wants the
            // Windows certificate store; bundled webpki roots are rejected as user-specified.
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build(),
    );
    builder.build().into()
}

fn build_payload(inv: &Inventory, bw_gbps: f64, gflops: f64, calibration: &Option<Calibration>, cond: &Conditions) -> String {
    let bucket = |x: f64, w: f64| (x / w).round() * w;
    let gpu_desc = gpu_description(inv);
    let class_input = format!(
        "sjv1n|{}|{}|{}|{}|{}|{}|{}",
        inv.os,
        inv.arch,
        inv.cpu_name,
        gpu_desc.clone().unwrap_or_default(),
        inv.logical_cores,
        inv.ram_total_gb.map(|g| g.round()).unwrap_or(0.0),
        bucket(bw_gbps, 5.0)
    );
    let class_hash = sha256_hex(class_input.as_bytes());

    let gpu_json = match &gpu_desc {
        Some(d) => format!("{{\"description\":{}}}", jstr(d)),
        None => "null".to_string(),
    };
    let runs_json = match calibration {
        Some(c) => format!(
            "[{{\"model\":{},\"file_gb\":{},\"tps\":{},\"predicted_tps\":{},\"threads\":{}}}]",
            jstr(&c.model_name),
            jnum(c.file_gb),
            jnum(c.measured_tps),
            jnum(c.ceiling_tps * DEFAULT_K),
            c.threads
        ),
        None => "[]".to_string(),
    };
    let k_json = match calibration {
        Some(c) if c.k_measured > 0.0 => jnum(c.k_measured),
        _ => "null".to_string(),
    };
    let mem_known = inv.ram_total_gb.is_some();
    let payload = format!(
        "{{\"class_hash\":{},\"source\":\"speedcheck\",\"platform\":{},\"arch\":{},\"gpu\":{},\"threads\":{},\"memory_gb\":{},\"memory_known\":{},\"bandwidth_gbps\":{},\"gflops_f32\":{},\"k\":{},\"on_ac\":{},\"power_mode\":{},\"cpu_load_pct\":{},\"runs\":{}}}",
        jstr(&class_hash),
        jstr(&inv.os),
        jstr(&inv.arch),
        gpu_json,
        inv.logical_cores,
        jopt(inv.ram_total_gb),
        mem_known,
        jnum(bw_gbps),
        jnum(gflops),
        k_json,
        match cond.on_ac { Some(true) => "true", Some(false) => "false", None => "null" },
        jstr(&cond.power_mode),
        match cond.cpu_load_pct { Some(l) => jnum(l), None => "null".to_string() },
        runs_json
    );
    payload
}

/// Send the receipt. Nothing leaves the machine until this is called, and it is only called
/// after an explicit yes (or a bare Enter, which defaults to yes).
fn submit_receipt(inv: &Inventory, bw_gbps: f64, gflops: f64, calibration: &Option<Calibration>, cond: &Conditions, already_shown: bool) -> Result<String, String> {
    let payload = build_payload(inv, bw_gbps, gflops, calibration, cond);
    if !already_shown && std::env::var("SJ_DEBUG").is_ok() {
        eprintln!("payload: {}", payload);
    }
    let agent = http_agent(Duration::from_secs(20));
    let mut resp = agent
        .post("https://saphojuice.com/v1/receipt")
        .content_type("application/json")
        .send(payload.as_str())
        .map_err(|e| format!("{e}"))?;
    resp.body_mut().read_to_string().map_err(|e| format!("{e}"))
}

// ---------------------------------------------------------------- run conditions
//
// The same machine measured 20.3 GB/s and 34.6 GB/s within an hour. Neither number is wrong;
// they were taken under different conditions. A dataset worth licensing has to record the
// conditions, not just the result, or rows cannot be compared with each other.

struct Conditions {
    on_ac: Option<bool>,     // None when the machine has no battery at all (a desktop)
    power_mode: String,      // the OS power plan or profile, verbatim
    cpu_load_pct: Option<f64>,   // total load just before measuring
    has_battery: bool,
}

/// Windows 11 has two power layers and they disagree.
///
/// The legacy power plan (`powercfg /getactivescheme`) usually reads "Balanced" and stays there.
/// What people actually set, the power mode slider in Settings, is stored separately as an
/// "overlay" scheme. A machine set to Best performance still reports the Balanced plan, so
/// reading only the plan records the wrong mode on essentially every Windows 11 laptop.
///
/// powercfg has no working flag for the overlay on current builds: /getactiveoverlayscheme,
/// /overlaysetting and /overlay all return "Invalid Parameters". Read it from the registry,
/// which is where powercfg itself keeps it.
#[cfg(windows)]
fn windows_power_mode(on_ac: Option<bool>, plan: &str) -> String {
    const KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes";
    let value = if on_ac == Some(false) { "ActiveOverlayDcPowerScheme" } else { "ActiveOverlayAcPowerScheme" };
    let guid = run("reg", &["query", KEY, "/v", value])
        .and_then(|s| s.split_whitespace().last().map(|g| g.trim().to_ascii_lowercase()));
    match guid.as_deref() {
        Some("ded574b5-45a0-4f42-8737-46345c09c238") => "Best performance".to_string(),
        Some("3af9b8d9-7c97-431d-ad78-34a8bfea439f") => "Better performance".to_string(),
        Some("961cc777-2547-4f9d-8174-7d86181b8a7a") => "Best power efficiency".to_string(),
        // all zeroes means no overlay is applied, so the legacy plan really is the mode
        Some("00000000-0000-0000-0000-000000000000") | None => plan.to_string(),
        Some(other) => format!("{} (overlay {})", plan, other),
    }
}

fn conditions() -> Conditions {
    let mut on_ac = None;
    let mut power_mode = "unknown".to_string();
    let mut cpu_load_pct = None;
    let mut has_battery = false;

    #[cfg(windows)]
    {
        // BatteryStatus: 1 = discharging, 2 = on AC. Absent entirely on a desktop.
        if let Some(v) = ps("$b=Get-CimInstance Win32_Battery -EA SilentlyContinue; if($b){($b|Select-Object -First 1).BatteryStatus}else{'none'}") {
            let v = v.trim().to_string();
            if v == "none" || v.is_empty() {
                has_battery = false;
                on_ac = Some(true);          // no battery means it is mains powered
            } else {
                has_battery = true;
                on_ac = Some(v != "1");
            }
        }
        let mut plan = "unknown".to_string();
        if let Some(v) = run("powercfg", &["/getactivescheme"]) {
            // "Power Scheme GUID: <guid>  (Balanced)" -> "Balanced"
            plan = v.rsplit('(').next().unwrap_or("").trim_end_matches(")
").trim_end_matches(')').trim().to_string();
            if plan.is_empty() { plan = v.trim().to_string(); }
        power_mode = windows_power_mode(on_ac, &plan);
        }
        if let Some(v) = ps("(Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average") {
            cpu_load_pct = v.trim().parse::<f64>().ok();
        }
    }

    #[cfg(target_os = "linux")]
    {
        for n in 0..4 {
            if let Some(v) = read_file(&format!("/sys/class/power_supply/AC{}/online", n))
                .or_else(|| read_file(&format!("/sys/class/power_supply/ADP{}/online", n))) {
                on_ac = Some(v.trim() == "1");
                break;
            }
        }
        has_battery = read_file("/sys/class/power_supply/BAT0/status").is_some();
        if !has_battery && on_ac.is_none() { on_ac = Some(true); }
        if let Some(v) = read_file("/sys/devices/system/cpu/cpufreq/policy0/scaling_governor") {
            power_mode = v.trim().to_string();
        }
        // load average over the last minute, as a percentage of all cores
        if let Some(v) = read_file("/proc/loadavg") {
            if let Some(first) = v.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()) {
                let cores = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1) as f64;
                cpu_load_pct = Some((first / cores * 100.0).min(100.0));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(v) = run("pmset", &["-g", "batt"]) {
            has_battery = v.contains("InternalBattery");
            on_ac = Some(v.contains("AC Power"));
            if !has_battery { on_ac = Some(true); }
        }
        if let Some(v) = run("pmset", &["-g", "therm"]) {
            if v.contains("CPU_Speed_Limit") { power_mode = v.lines().find(|l| l.contains("CPU_Speed_Limit")).unwrap_or("").trim().to_string(); }
        }
        if let Some(v) = run("sh", &["-c", "sysctl -n vm.loadavg | awk '{print $2}'"]) {
            if let Ok(first) = v.trim().parse::<f64>() {
                let cores = std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1) as f64;
                cpu_load_pct = Some((first / cores * 100.0).min(100.0));
            }
        }
    }

    Conditions { on_ac, power_mode, cpu_load_pct, has_battery }
}

/// Anything that makes this run not comparable with a clean one. Empty means conditions were fine.
fn degraded(c: &Conditions) -> Vec<String> {
    let mut w = Vec::new();
    if c.has_battery && c.on_ac == Some(false) {
        w.push("Running on battery; plug in for a full-speed result.".to_string());
    }
    let m = c.power_mode.to_ascii_lowercase();
    if m.contains("saver") || m.contains("powersave") || m.contains("eco") || m.contains("efficiency") {
        w.push(format!("Power mode is \"{}\"; set it to Balanced or Best Performance for a full-speed result.", c.power_mode));
    }
    if let Some(l) = c.cpu_load_pct {
        if l >= 25.0 {
            w.push(format!("Other software is using about {:.0}% of the CPU; close it for a comparable result.", l));
        }
    }
    w
}

// ---------------------------------------------------------------- reference models and derivation

struct RefModel {
    name: &'static str,
    total_gb: f64,        // Q4_K_M file size, from the Hugging Face API
    active_gb: f64,       // bytes read per token (== total for dense, active experts for MoE)
    kv_kb_per_tok: f64,   // f16 KV cache bytes per token / 1024
    params_active_b: f64, // active params in billions (for prefill flops)
    hf: &'static str,     // Hugging Face repo
    gguf: &'static str,   // exact file in that repo
    quant: &'static str,
    lic: &'static str,
    ollama: &'static str, // one command, if Ollama is installed
}

fn reference_models() -> Vec<RefModel> {
    // The same five models the site grades, with repo, quant and size checked against the
    // Hugging Face API and the Ollama registry on 20 September 2026. Keep in step with
    // worker/catalog.js and the CAT array in site/index.html.
    vec![
        RefModel { name: "Qwen3 1.7B", total_gb: 1.11, active_gb: 1.11, kv_kb_per_tok: 112.0, params_active_b: 1.7,
                   hf: "unsloth/Qwen3-1.7B-GGUF", gguf: "Qwen3-1.7B-Q4_K_M.gguf", quant: "Q4_K_M", lic: "Apache 2.0", ollama: "qwen3:1.7b" },
        RefModel { name: "Qwen3 4B", total_gb: 2.50, active_gb: 2.50, kv_kb_per_tok: 144.0, params_active_b: 4.0,
                   hf: "unsloth/Qwen3-4B-GGUF", gguf: "Qwen3-4B-Q4_K_M.gguf", quant: "Q4_K_M", lic: "Apache 2.0", ollama: "qwen3:4b" },
        RefModel { name: "Qwen3 8B", total_gb: 5.03, active_gb: 5.03, kv_kb_per_tok: 128.0, params_active_b: 8.0,
                   hf: "unsloth/Qwen3-8B-GGUF", gguf: "Qwen3-8B-Q4_K_M.gguf", quant: "Q4_K_M", lic: "Apache 2.0", ollama: "qwen3:8b" },
        RefModel { name: "Qwen3 14B", total_gb: 9.00, active_gb: 9.00, kv_kb_per_tok: 160.0, params_active_b: 14.0,
                   hf: "unsloth/Qwen3-14B-GGUF", gguf: "Qwen3-14B-Q4_K_M.gguf", quant: "Q4_K_M", lic: "Apache 2.0", ollama: "qwen3:14b" },
        RefModel { name: "Qwen3 30B-A3B", total_gb: 18.56, active_gb: 2.00, kv_kb_per_tok: 96.0, params_active_b: 3.3,
                   hf: "unsloth/Qwen3-30B-A3B-GGUF", gguf: "Qwen3-30B-A3B-Q4_K_M.gguf", quant: "Q4_K_M MoE", lic: "Apache 2.0", ollama: "qwen3:30b-a3b" },
    ]
}

// ---------------------------------------------------------------- main

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "k" {
        k_subcommand(&args[2..]);
        return;
    }
    let opts = match parse_opts() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    };

    eprintln!("SAPHOJUICE probe v{}", VERSION);
    eprintln!("[1/8] inventory");
    // before anything is measured, so the numbers can be compared with other runs later
    let cond = conditions();
    let warnings = degraded(&cond);
    let inv = inventory();
    eprintln!("  {} | {} | {} cores | isa {:?}", inv.cpu_name, inv.arch, inv.logical_cores, inv.isa);
    if let Some(t) = inv.ram_total_gb {
        eprintln!("  RAM total {:.1} GB, available {:.1} GB", t, inv.ram_available_gb.unwrap_or(0.0));
    }
    for n in &inv.raw_notes {
        eprintln!("  note: {}", n);
    }

    eprintln!("[2/8] bandwidth sweep (STREAM triad)");
    let (bw_multi, bw_single) = bandwidth_sweep(inv.logical_cores, inv.ram_available_gb, opts.quick);
    let dram_bw = bw_multi.last().map(|p| p.gbps).unwrap_or(0.0);
    let dram_bw_single = bw_single.last().map(|p| p.gbps).unwrap_or(0.0);
    let peak_cache_bw = bw_multi.iter().map(|p| p.gbps).fold(0.0, f64::max);
    let thread_bw = thread_sweep(inv.logical_cores, opts.quick);
    for (c, g) in &thread_bw { eprintln!("  {} thread(s)  {:.1} GB/s", c, g); }

    eprintln!("[3/8] compute");
    let gemm_budget = if opts.quick { Duration::from_secs(2) } else { Duration::from_secs(4) };
    let gemm_mt = gemm_gflops(512, inv.logical_cores, gemm_budget);
    eprintln!("  f32 GEMM 512^3 x{} threads  {:.1} GFLOPS", inv.logical_cores, gemm_mt);
    let gemm_st = gemm_gflops(384, 1, gemm_budget / 2);
    eprintln!("  f32 GEMM 384^3 x1 thread    {:.1} GFLOPS", gemm_st);
    let i8_mt = int8_gops(inv.logical_cores, Duration::from_secs(2));
    eprintln!("  int8 dot x{} threads        {:.1} Gops", inv.logical_cores, i8_mt);

    eprintln!("[4/8] sustained ({}s bandwidth, {}s compute)", opts.bw_seconds, opts.gemm_seconds);
    let big_per_thread = bw_multi.last().map(|p| p.bytes_per_thread).unwrap_or(16 * 1024 * 1024);
    let cores = inv.logical_cores;
    let bw_series = sustained(opts.bw_seconds, 5, |d| triad_bw(big_per_thread, cores, d), "bw");
    let gemm_series = sustained(opts.gemm_seconds, 5, |d| gemm_gflops(512, cores, d), "gemm");
    // peak = best observed anywhere (sweep or first sustained windows); sustained = mean of the last two windows
    let bw_sustained = bw_series.iter().rev().take(2).sum::<f64>() / (bw_series.len().min(2) as f64);
    let gemm_sustained = gemm_series.iter().rev().take(2).sum::<f64>() / (gemm_series.len().min(2) as f64);
    let dram_bw = bw_series.iter().cloned().fold(dram_bw, f64::max);
    let gemm_mt = gemm_series.iter().cloned().fold(gemm_mt, f64::max);
    let bw_decay = if dram_bw > 0.0 { (1.0 - bw_sustained / dram_bw).max(0.0) } else { 0.0 };
    let gemm_decay = if gemm_mt > 0.0 { (1.0 - gemm_sustained / gemm_mt).max(0.0) } else { 0.0 };

    eprintln!("[5/8] storage");
    let storage = if opts.skip_storage { None } else { storage_test() };
    if let Some((w, r)) = storage {
        eprintln!("  write {:.2} GB/s, read {:.2} GB/s (512 MB, read may be page-cached)", w, r);
    }

    eprintln!("[6/8] model calibration");
    let ram_total = inv.ram_total_gb.unwrap_or(0.0);
    let bw_for_pred = if bw_sustained > 0.0 { bw_sustained } else { dram_bw };
    let calibration: Option<Calibration> = if opts.offline {
        eprintln!("  --offline: skipping model download and generation run");
        None
    } else {
        match run_calibration(&inv, bw_for_pred) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("  skipped: {}", e);
                None
            }
        }
    };
    let (k_used, k_calibrated) = match &calibration {
        Some(c) if c.k_measured > 0.0 => (c.k_measured, true),
        _ => (DEFAULT_K, false),
    };
    let calibration_json = match &calibration {
        Some(c) => format!(
            "{{\"model\":{},\"file_gb\":{},\"engine\":{},\"threads\":{},\"n_gen_tokens\":{},\"measured_tps\":{},\"ceiling_tps\":{},\"k_measured\":{}}}",
            jstr(&c.model_name), jnum(c.file_gb), jstr(&c.engine_path), c.threads, c.n_gen, jnum(c.measured_tps), jnum(c.ceiling_tps), jnum(c.k_measured)
        ),
        None => "null".to_string(),
    };

    eprintln!("[7/8] derive");
    let mut pred_json = Vec::new();
    let free_gb = inv.ram_available_gb.unwrap_or(0.0);
    let k_report = calibration.as_ref().map(|c| c.k_measured).filter(|k| *k > 0.0).unwrap_or(DEFAULT_K);
    let k_is_measured = calibration.as_ref().map(|c| c.k_measured > 0.0).unwrap_or(false);

    println!();
    println!("MACHINE  (measured on this computer just now)");
    println!("  CPU            {}", inv.cpu_name);
    println!("  Cores          {} logical", inv.logical_cores);
    match (inv.ram_total_gb, inv.ram_available_gb) {
        (Some(t), Some(a)) => println!("  Memory         {:.1} GB total, {:.1} GB free right now", t, a),
        (Some(t), None)    => println!("  Memory         {:.1} GB total, free memory unknown", t),
        _                  => println!("  Memory         unknown"),
    }
    println!("  Instructions   {}", if inv.isa.is_empty() { "none detected".to_string() } else { inv.isa.join(", ") });
    println!("  Conditions     {}{}{}",
             match (cond.has_battery, cond.on_ac) {
                 (true, Some(true)) => "plugged in".to_string(),
                 (true, Some(false)) => "on battery".to_string(),
                 (false, _) => "mains powered".to_string(),
                 (true, None) => "power source unknown".to_string(),
             },
             if cond.power_mode != "unknown" { format!(", {}", cond.power_mode) } else { String::new() },
             match cond.cpu_load_pct { Some(l) => format!(", {:.0}% CPU busy at start", l), None => String::new() });
    let best_threads = thread_bw.iter().cloned().fold((0usize, 0f64), |a, b| if b.1 > a.1 { b } else { a });
    let sweep_desc: Vec<String> = thread_bw.iter().map(|(c, g)| format!("{}t {:.1}", c, g)).collect();
    println!("  Memory speed   {:.1} GB/s at {} thread{}   (measured at {})",
             best_threads.1, best_threads.0, if best_threads.0 == 1 { "" } else { "s" }, sweep_desc.join(", "));
    println!("  Sustained      {:.1} GB/s over {}s", bw_sustained, opts.bw_seconds);
    println!("  Compute        {:.1} GFLOP/s f32, {:.1} Gops int8", gemm_sustained.max(gemm_mt), i8_mt);
    println!("  Detail         DRAM {:.1} peak / {:.1} sustained GB/s ({:.0}% decay), 1 thread {:.1}, cache {:.0} GB/s",
             dram_bw, bw_sustained, bw_decay * 100.0, dram_bw_single, peak_cache_bw);

    println!();
    if k_is_measured {
        println!("MODELS  (tok/s measured from a real generation run on this machine, k = {:.2})", k_report);
    } else {
        println!("MODELS  (tok/s PREDICTED from the memory speed above, k = {:.2} assumed.", k_report);
        println!("         No model was generated: a bundled engine lands in a later release.)");
    }
    println!();

    // the recommended pick: the fastest model that fits in free memory right now
    let models = reference_models();
    let mut best_idx: Option<usize> = None;
    for (i, m) in models.iter().enumerate() {
        let kv = m.kv_kb_per_tok * CTX_TOKENS / 1024.0 / 1024.0;
        if free_gb > 0.0 && (m.total_gb + kv) <= free_gb {
            let tps = k_report * (if m.active_gb > 0.0 { bw_for_pred / m.active_gb } else { 0.0 });
            if tps >= 8.0 || best_idx.is_none() { best_idx = Some(i); }
        }
    }

    for (i, m) in models.iter().enumerate() {
        let ceiling = if m.active_gb > 0.0 { bw_for_pred / m.active_gb } else { 0.0 };
        let est = k_report * ceiling;
        let prefill = if m.params_active_b > 0.0 { (gemm_sustained.max(gemm_mt * 0.5) * 0.5) / (2.0 * m.params_active_b) } else { 0.0 };
        let kv_gb = m.kv_kb_per_tok * CTX_TOKENS / 1024.0 / 1024.0;
        let need = m.total_gb + kv_gb + OS_RESERVE_GB;
        let fits_now = free_gb > 0.0 && (m.total_gb + kv_gb) <= free_gb;
        let fits_total = ram_total > 0.0 && need <= ram_total;
        let mark = if Some(i) == best_idx { ">" } else { " " };
        let verdict = if free_gb <= 0.0 { "free memory unknown".to_string() }
            else if fits_now { "fits in free memory now".to_string() }
            else if fits_total { format!("needs {:.1} GB free, you have {:.1} GB", m.total_gb + kv_gb, free_gb) }
            else { format!("will not fit: needs {:.1} GB", m.total_gb + kv_gb) };
        let tps_str = format!("~{:.1}", est);
        println!("{} {:<14} {:>7} tok/s {}   {:>6.2} GB   {}",
                 mark, m.name, tps_str, if k_is_measured { "measured " } else { "predicted" }, m.total_gb, verdict);
        println!("      {} · {} · {} · {}", m.hf, m.gguf, m.quant, m.lic);
        println!("      ollama run {}", m.ollama);
        println!();
        pred_json.push(format!(
            "{{\"model\":{},\"hf\":{},\"gguf\":{},\"quant\":{},\"total_gb\":{},\"active_gb\":{},\"decode_ceiling_tps\":{},\"decode_est_tps\":{},\"prefill_est_tps\":{},\"kv_gb_at_ctx\":{},\"need_gb\":{},\"fits\":{},\"fits_free_now\":{}}}",
            jstr(m.name), jstr(m.hf), jstr(m.gguf), jstr(m.quant), jnum(m.total_gb), jnum(m.active_gb),
            jnum(ceiling), jnum(est), jnum(prefill), jnum(kv_gb), jnum(need), fits_total, fits_now
        ));
    }
    if !warnings.is_empty() {
        println!();
        for w in &warnings { println!("  NOTE: {}", w); }
        println!("  These numbers are a floor, not this machine's best.");
    }
    if best_idx.is_some() {
        println!("  > recommended: the fastest model that fits in your free memory right now.");
    } else {
        println!("  Nothing fits in the memory free right now. Close some applications and run this again.");
    }


    // ---- receipt JSON
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let bw_multi_json: Vec<String> = bw_multi.iter().map(|p| format!("{{\"working_set_mb\":{},\"threads\":{},\"gbps\":{}}}", jnum(p.bytes_per_thread as f64 * 3.0 * p.threads as f64 / 1e6), p.threads, jnum(p.gbps))).collect();
    let bw_single_json: Vec<String> = bw_single.iter().map(|p| format!("{{\"working_set_mb\":{},\"threads\":1,\"gbps\":{}}}", jnum(p.bytes_per_thread as f64 * 3.0 / 1e6), jnum(p.gbps))).collect();
    let bw_series_json: Vec<String> = bw_series.iter().map(|v| jnum(*v)).collect();
    let gemm_series_json: Vec<String> = gemm_series.iter().map(|v| jnum(*v)).collect();
    let isa_json: Vec<String> = inv.isa.iter().map(|s| jstr(s)).collect();
    let notes_json: Vec<String> = inv.raw_notes.iter().map(|s| jstr(s)).collect();
    let (st_w, st_r) = match storage { Some((w, r)) => (Some(w), Some(r)), None => (None, None) };

    let receipt = format!(
        "{{\n  \"receipt_version\": \"0.1\",\n  \"probe_version\": {},\n  \"timestamp_unix\": {},\n  \"machine\": {{\n    \"os\": {},\n    \"arch\": {},\n    \"host_hash\": {},\n    \"cpu\": {},\n    \"logical_cores\": {},\n    \"isa\": [{}],\n    \"ram_total_gb\": {},\n    \"ram_available_gb\": {},\n    \"ram_modules\": {},\n    \"gpu\": {},\n    \"power_plan\": {},\n    \"battery\": {}\n  }},\n  \"bandwidth\": {{\n    \"sweep_multithread\": [{}],\n    \"sweep_singlethread\": [{}],\n    \"dram_peak_gbps\": {},\n    \"dram_sustained_gbps\": {},\n    \"dram_single_thread_gbps\": {},\n    \"peak_cache_gbps\": {},\n    \"sustained_series_gbps\": [{}],\n    \"sustained_window_s\": 5,\n    \"decay_fraction\": {}\n  }},\n  \"compute\": {{\n    \"f32_gemm_peak_gflops\": {},\n    \"f32_gemm_single_thread_gflops\": {},\n    \"f32_gemm_sustained_gflops\": {},\n    \"sustained_series_gflops\": [{}],\n    \"decay_fraction\": {},\n    \"int8_dot_gops\": {}\n  }},\n  \"storage\": {{\n    \"seq_write_gbps\": {},\n    \"seq_read_gbps\": {},\n    \"note\": \"512 MB sequential in temp dir; read may be served from page cache\"\n  }},\n  \"derived\": {{\n    \"k_used\": {},\n    \"k_calibrated\": {},\n    \"ctx_tokens\": {},\n    \"os_reserve_gb\": {},\n    \"predictions\": [{}]\n  }},\n  \"calibration\": {},\n  \"notes\": [{}]\n}}\n",
        jstr(VERSION), now,
        jstr(&inv.os), jstr(&inv.arch), jstr(&inv.hostname_hash), jstr(&inv.cpu_name), inv.logical_cores, isa_json.join(","),
        jopt(inv.ram_total_gb), jopt(inv.ram_available_gb), inv.ram_modules_json, inv.gpu_json, jstr(&inv.power_plan), inv.battery_json,
        bw_multi_json.join(","), bw_single_json.join(","), jnum(dram_bw), jnum(bw_sustained), jnum(dram_bw_single), jnum(peak_cache_bw), bw_series_json.join(","), jnum(bw_decay),
        jnum(gemm_mt), jnum(gemm_st), jnum(gemm_sustained), gemm_series_json.join(","), jnum(gemm_decay), jnum(i8_mt),
        jopt(st_w), jopt(st_r),
        jnum(k_used), k_calibrated, CTX_TOKENS as u64, jnum(OS_RESERVE_GB), pred_json.join(","),
        calibration_json,
        notes_json.join(",")
    );
    // Nothing is written to disk unless asked. A one-liner run must leave the machine as it
    // found it, and dropping a JSON file in whatever directory the user happened to be in is
    // not that.
    if !opts.out.is_empty() {
        match std::fs::write(&opts.out, receipt.as_bytes()) {
            Ok(_) => println!("Full receipt written to {}", opts.out),
            Err(e) => eprintln!("could not write receipt: {}", e),
        }
    }

    eprintln!("[8/8] share");
    if opts.show_payload {
        println!();
        println!("This is exactly what would be sent:");
        println!("{}", build_payload(&inv, bw_for_pred, gemm_sustained, &calibration, &cond));
        println!();
    }
    if opts.no_share {
        println!();
        println!("--no-share: nothing was sent.");
        return;
    }
    println!();
    println!("Share your results with the SAPHOJUICE community?");
    println!("Hardware and speed numbers only, never personal info. [Y/n]");
    println!("Terms: saphojuice.com/terms");
    print!("> ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    // Default yes means a bare Enter shares. End of input is NOT a bare Enter: it means nobody
    // was there to answer, which is not consent, so that case sends nothing.
    let share = match std::io::stdin().read_line(&mut answer) {
        Ok(0) => {
            println!();
            println!("No answer possible (no terminal attached), so nothing was sent.");
            println!("Run it again from a terminal, or pass --no-share to skip this prompt.");
            false
        }
        Ok(_) => !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no"),
        Err(_) => false,
    };
    if !share {
        if !answer.is_empty() { println!("Not sent. Your numbers stayed on this machine."); }
        return;
    }
    match submit_receipt(&inv, bw_for_pred, gemm_sustained, &calibration, &cond, opts.show_payload) {
        Ok(body) => match json_str(&body, "id") {
            Some(id) => {
                println!();
                println!("Thank you. Your result is on the public table.");
                println!("Share it:  https://saphojuice.com/r/{}", id);
                // The only proof this row is yours. Shown once, never published, and the sole
                // way to delete it later: class_hash is public and cannot authorise deletion.
                match json_str(&body, "delete_token") {
                    Some(tok) => {
                        println!();
                        println!("Keep this if you may want it removed later. It is shown once:");
                        println!("  delete token: {}", tok);
                        println!("  to remove:    curl -X POST https://saphojuice.com/v1/delete \\");
                        println!("                  -H 'content-type: application/json' \\");
                        println!("                  -d '{{\"delete_token\":\"{}\"}}'", tok);
                    }
                    None => println!("(no delete token returned; contact hello@saphojuice.com to remove it)"),
                }
            }
            None => println!("Sent. {}", body),
        },
        Err(e) => eprintln!("Could not send: {}", e),
    }
}

fn k_subcommand(args: &[String]) {
    let mut tokps = None;
    let mut model_gb = None;
    let mut bw = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tokps" => { i += 1; tokps = args.get(i).and_then(|s| s.parse::<f64>().ok()); }
            "--model-gb" => { i += 1; model_gb = args.get(i).and_then(|s| s.parse::<f64>().ok()); }
            "--bw" => { i += 1; bw = args.get(i).and_then(|s| s.parse::<f64>().ok()); }
            _ => {}
        }
        i += 1;
    }
    match (tokps, model_gb, bw) {
        (Some(t), Some(m), Some(b)) if m > 0.0 && b > 0.0 => {
            let ceiling = b / m;
            let k = t / ceiling;
            println!("ceiling {:.1} tok/s, measured {:.1} tok/s, k = {:.3}", ceiling, t, k);
            if k > 1.05 {
                println!("k > 1: model bytes or bandwidth is wrong (check file size, or GPU is doing the work).");
            } else if k < 0.3 {
                println!("k < 0.3: something is misconfigured (wrong build, thread count, power plan, or RAM spill).");
            }
        }
        _ => println!("usage: sj-probe k --tokps <measured tg tok/s> --model-gb <model file GB> --bw <GB/s from receipt>"),
    }
}
