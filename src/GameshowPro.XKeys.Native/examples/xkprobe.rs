// Copyright (c) Barjonas LLC 2026.
//! `xkprobe`: diagnostic tool for `xkhid`, calling the same exports the managed binding uses.
//!
//! ```text
//! cargo run --release --example xkprobe -- list
//! cargo run --release --example xkprobe -- probe [--requests N] [--gap-ms G] [--seconds S] [--pid 0x0426] [--mmcss]
//!                                                [--offset K] [--show-be 9,10]
//! cargo run --release --example xkprobe -- leak [--iterations 100000] [--vid 0x05F3] [--usage-page 0x0C] [--output-len 36]
//! cargo run --release --example xkprobe -- watch [--seconds S]
//! cargo run --release --example xkprobe -- guide [--help]
//! ```
//!
//! `guide` is the guided manual test for a person at the devices (see `guide --help`); its logic lives in `src/guide.rs`.
//!
//! `probe` opens the consumer-control interface (usage page 0x0C) of every P.I. Engineering device and sends it only the
//! Generate Data output report (`[0, 177, 0, ...]`), which makes the device send one input report without a key being pressed.
//! It sends no other output report. Each report is printed in hex with its QPC stamp in microseconds since the start.

use std::time::{Duration, Instant};
use xkhid::*;

use core::ffi::c_void;

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCountersEx {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(process: *mut c_void, counters: *mut ProcessMemoryCountersEx, cb: u32) -> i32;
    fn GetProcessHandleCount(process: *mut c_void, count: *mut u32) -> i32;
}

/// The only output report this tool ever sends: Generate Data (P.I. SDK command 177).
const GENERATE_DATA: u8 = 177;

fn qpc_frequency() -> i64 {
    let mut f = 0;
    // SAFETY: valid out pointer.
    unsafe { win32::QueryPerformanceFrequency(&mut f) };
    f
}

fn private_bytes_and_handles() -> (usize, u32) {
    let mut counters = ProcessMemoryCountersEx { cb: size_of::<ProcessMemoryCountersEx>() as u32, ..Default::default() };
    let mut handles = 0u32;
    // SAFETY: valid out pointers; the process pseudo-handle needs no closing.
    unsafe {
        let process = win32::GetCurrentProcess();
        K32GetProcessMemoryInfo(process, &mut counters, counters.cb);
        GetProcessHandleCount(process, &mut handles);
    }
    (counters.private_usage, handles)
}

fn wide_to_string(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

struct Args(Vec<String>);

impl Args {
    fn value(&self, name: &str) -> Option<&str> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).map(String::as_str)
    }
    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn number(&self, name: &str, default: i64) -> i64 {
        match self.value(name) {
            None => default,
            Some(v) => {
                let parsed = if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                    i64::from_str_radix(h, 16)
                } else {
                    v.parse()
                };
                parsed.unwrap_or_else(|_| panic!("{name}: not a number: {v}"))
            }
        }
    }
}

fn enumerate(vid: u16, usage_page: u16, output_len: u16) -> Vec<DeviceInfo> {
    let mut out = vec![DeviceInfo::ZERO; 64];
    let mut count = 0u32;
    // SAFETY: `out` holds 64 entries; `count` is writable.
    let result = unsafe { xkhid_enumerate(vid, usage_page, output_len, out.as_mut_ptr(), out.len() as u32, &mut count) };
    assert!(result == XKHID_OK || result == XKHID_E_BUFFER_TOO_SMALL, "xkhid_enumerate failed: {result}");
    out.truncate((count as usize).min(out.len()));
    out
}

fn list() {
    let found = enumerate(PI_ENGINEERING_VID, 0, 0);
    println!("{} P.I. Engineering HID interface(s):", found.len());
    for d in &found {
        println!(
            "  VID {:04X} PID {:04X} ver {:04X}  usage page {:04X} usage {:04X}  in {} out {} feature {}  \"{}\" / \"{}\"\n    {}",
            d.vid,
            d.pid,
            d.version,
            d.usage_page,
            d.usage,
            d.input_len,
            d.output_len,
            d.feature_len,
            wide_to_string(&d.manufacturer),
            wide_to_string(&d.product),
            wide_to_string(&d.path)
        );
    }
}

fn probe(args: &Args) {
    let requests = args.number("--requests", 5).max(0) as usize;
    let gap_ms = args.number("--gap-ms", 500).max(0) as u64;
    let seconds = args.number("--seconds", 0) as f64;
    let only_pid = args.number("--pid", -1);
    let offset_override = args.value("--offset").map(|_| args.number("--offset", -1) as i32);
    let show_be: Vec<usize> =
        args.value("--show-be").map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default();
    let flags = if args.flag("--mmcss") { OPEN_MMCSS } else { 0 };
    let frequency = qpc_frequency() as f64;

    let devices: Vec<DeviceInfo> = enumerate(PI_ENGINEERING_VID, 0x000C, 0)
        .into_iter()
        .filter(|d| d.input_len > 0 && d.output_len > 0 && (only_pid < 0 || i64::from(d.pid) == only_pid))
        .collect();
    if devices.is_empty() {
        println!("No P.I. Engineering consumer-control interface with input and output reports found.");
        return;
    }
    for info in devices {
        let offset = offset_override.unwrap_or_else(|| xkhid_timestamp_offset(info.pid));
        println!();
        println!(
            "== PID {:04X} \"{}\" in {} out {}; timestamp offset {offset}; {requests} Generate Data request(s), {gap_ms} ms apart{}",
            info.pid,
            wide_to_string(&info.product),
            info.input_len,
            info.output_len,
            if flags & OPEN_MMCSS != 0 { "; MMCSS" } else { "" }
        );
        let mut device = core::ptr::null_mut();
        // SAFETY: the path is NUL-terminated; `device` is a valid out pointer.
        let result = unsafe { xkhid_open(info.path.as_ptr(), 4096, flags, offset, &mut device) };
        if result != XKHID_OK {
            println!("   open failed: {result}");
            continue;
        }
        let t0 = win32::qpc();
        let us = |ticks: i64| (ticks - t0) as f64 * 1e6 / frequency;
        let mut report = vec![0u8; info.output_len as usize];
        report[1] = GENERATE_DATA;
        let mut writes = Vec::new();
        for i in 0..requests {
            let before = win32::qpc();
            let mut win32_error = 0u32;
            // SAFETY: `device` is live; `report` is output_len bytes.
            let result = unsafe { xkhid_write(device, report.as_ptr(), report.len() as u32, 1000, &mut win32_error) };
            let after = win32::qpc();
            writes.push((before, after, result, win32_error));
            if i + 1 < requests && gap_ms > 0 {
                std::thread::sleep(Duration::from_millis(gap_ms));
            }
        }
        // Let the last answers arrive, then keep reading until `seconds` have passed since the start.
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end || us(win32::qpc()) < seconds * 1e6 {
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut entries = vec![ReportEntry::ZERO; 4096];
        // SAFETY: `device` is live; `entries` holds 4096; this is the only reader.
        let count = unsafe { xkhid_read(device, entries.as_mut_ptr(), entries.len() as i32) } as usize;
        // SAFETY: as above.
        let dropped = unsafe { xkhid_dropped(device) };
        // SAFETY: `device` is live and nothing else uses it.
        unsafe { xkhid_close(device) };
        entries.truncate(count);

        for (i, (before, after, result, error)) in writes.iter().enumerate() {
            println!(
                "   write {i}: start {:>12.1} us, took {:>7.1} us, result {result}{}",
                us(*before),
                (after - before) as f64 * 1e6 / frequency,
                if *error != 0 { format!(" (Win32 {error})") } else { String::new() }
            );
        }
        println!("   {count} entr(ies), {dropped} dropped");
        let mut previous: Option<&ReportEntry> = None;
        let mut intervals = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let delta = previous.map(|p| (e.qpc - p.qpc) as f64 * 1e6 / frequency);
            let device_delta = previous
                .filter(|p| p.flags & e.flags & FLAG_HAS_DEVICE_MS != 0)
                .map(|p| i64::from(e.device_ms.wrapping_sub(p.device_ms)));
            if let Some(d) = delta {
                intervals.push(d);
            }
            let after_write = writes.get(i).map(|w| (e.qpc - w.1) as f64 * 1e6 / frequency);
            let be: String = show_be
                .iter()
                .filter_map(|&o| e.data.get(o..o + 4).map(|b| format!(" be@{o}={}", u32::from_be_bytes(b.try_into().unwrap()))))
                .collect();
            println!(
                "   #{:<4} t {:>12.1} us  dt {:>10}  flags {:03b}{}  dev_ms {:>10}  ddev {:>6}{}{}\n         {}",
                e.sequence,
                us(e.qpc),
                delta.map_or("-".into(), |d| format!("{d:.1}")),
                e.flags,
                if e.flags & FLAG_ERROR != 0 { format!(" ERROR {} (Win32 {})", e.error, e.win32_error) } else { String::new() },
                if e.flags & FLAG_HAS_DEVICE_MS != 0 { e.device_ms.to_string() } else { "-".into() },
                device_delta.map_or("-".into(), |d| d.to_string()),
                after_write.map_or(String::new(), |a| format!("  after write {i} end: {a:.1} us")),
                be,
                hex(&e.data[..e.length as usize])
            );
            previous = Some(e);
        }
        if !intervals.is_empty() {
            let mut sorted = intervals.clone();
            sorted.sort_by(f64::total_cmp);
            println!(
                "   intervals between consecutive reports (us): min {:.1}  median {:.1}  max {:.1}",
                sorted[0],
                sorted[sorted.len() / 2],
                sorted[sorted.len() - 1]
            );
        }
        let reports: Vec<&ReportEntry> = entries.iter().filter(|e| e.flags & FLAG_ERROR == 0).collect();
        if reports.len() >= 2 {
            let (first, last) = (reports[0], reports[reports.len() - 1]);
            if first.flags & last.flags & FLAG_HAS_DEVICE_MS != 0 {
                let device_span = f64::from(last.device_ms.wrapping_sub(first.device_ms));
                let host_span = (last.qpc - first.qpc) as f64 * 1e3 / frequency;
                println!(
                    "   device clock: {device_span:.0} ms over {host_span:.3} ms of QPC (ratio {:.5})",
                    device_span / host_span
                );
            }
            // How far each interval between consecutive reports is from a whole number of 1 ms or 8 ms periods: a narrow spread
            // means the reports arrive on that grid (the host's polling of the interrupt IN endpoint), and the spread is the
            // stamp jitter. Intervals rather than absolute times, so the slow drift between QPC and the USB frame clock cancels.
            for grid in [1000.0f64, 8000.0] {
                let residuals: Vec<f64> = reports
                    .windows(2)
                    .map(|pair| {
                        let t = (pair[1].qpc - pair[0].qpc) as f64 * 1e6 / frequency;
                        (t + grid / 2.0).rem_euclid(grid) - grid / 2.0
                    })
                    .collect();
                let min = residuals.iter().copied().fold(f64::MAX, f64::min);
                let max = residuals.iter().copied().fold(f64::MIN, f64::max);
                let mean = residuals.iter().sum::<f64>() / residuals.len() as f64;
                let sd = (residuals.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / residuals.len() as f64).sqrt();
                println!("   intervals vs a {grid:.0} us grid: residual min {min:+.1} max {max:+.1} sd {sd:.1} us");
            }
        }
    }
}

fn leak(args: &Args) {
    let iterations = args.number("--iterations", 100_000).max(1) as u64;
    let vid = args.number("--vid", i64::from(PI_ENGINEERING_VID)) as u16;
    let usage_page = args.number("--usage-page", 0x0C) as u16;
    let output_len = args.number("--output-len", 36) as u16;
    let mut out = vec![DeviceInfo::ZERO; 16];
    let mut run = |n: u64| -> u32 {
        let mut count = 0;
        for _ in 0..n {
            // SAFETY: `out` holds 16 entries; `count` is writable.
            unsafe { xkhid_enumerate(vid, usage_page, output_len, out.as_mut_ptr(), out.len() as u32, &mut count) };
        }
        count
    };
    // Warm up so one-time allocations (SetupAPI, the loader, the heap) are not counted as growth.
    let matched = run(1000);
    let (private_before, handles_before) = private_bytes_and_handles();
    println!(
        "leak test: {iterations} enumerations, VID {vid:04X} usage page {usage_page:04X} output length {output_len}; {matched} interface(s) match"
    );
    println!("  before: private bytes {private_before}, handles {handles_before}");
    let start = Instant::now();
    let step = (iterations / 10).max(1);
    let mut done = 0;
    while done < iterations {
        let n = step.min(iterations - done);
        run(n);
        done += n;
        let (private, handles) = private_bytes_and_handles();
        println!(
            "  after {done:>7}: private bytes {private} ({:+}), handles {handles} ({:+}), {:.1} s, {:.0} us per enumeration",
            private as i64 - private_before as i64,
            i64::from(handles) - i64::from(handles_before),
            start.elapsed().as_secs_f64(),
            start.elapsed().as_secs_f64() * 1e6 / done as f64
        );
    }
}

fn watch(args: &Args) {
    let seconds = args.number("--seconds", 10).max(0) as u64;
    let watch = xkhid_watch_create(0);
    assert!(!watch.is_null(), "xkhid_watch_create failed");
    // SAFETY: `watch` is live until destroyed; the event duplicate is closed at the end.
    unsafe {
        let event = xkhid_watch_event_handle(watch);
        println!("watching P.I. Engineering HID interfaces for {seconds} s; plug or unplug a device");
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut seen = xkhid_watch_generation(watch);
        while Instant::now() < end {
            if win32::WaitForSingleObject(event, 500) == win32::WAIT_OBJECT_0 {
                let generation = xkhid_watch_generation(watch);
                println!("  generation {seen} -> {generation}; {} consumer interface(s) now", enumerate(0, 0x0C, 0).len());
                seen = generation;
            }
        }
        xkhid_watch_destroy(watch);
        win32::CloseHandle(event);
    }
}

// ---- guide: the guided manual test ----------------------------------------------------------------------------------------------

mod guided {
    use super::*;
    use std::io::BufRead;
    use xkhid::guide::{self, Command, DeviceDesc, Hub, Input, Options, ScriptedInput, Selection, SessionOutcome, SimHub, StepResult};
    use xkhid::lights::{validate_light_report, LightModel};

    const HELP: &str = "\
xkprobe guide: a guided test of the X-keys devices plugged into this computer.

You run it, it tells you one thing at a time to do with a device (press a key, turn the jog dial, unplug the cable), watches what
the device sends, says what it saw and whether that is right, and goes on to the next thing. When it is done it writes a result file,
xkprobe-guide-<UTC date and time>.json, in the current folder (or the folder given with --out). The file is rewritten after every
step, so it exists even if you stop early.

  cargo run --release --example xkprobe -- guide [options]

What you will be asked, in order, for each device found:
  Jog & Shuttle   press each of the 12 keys; turn the jog dial both ways; move the shuttle ring a notch at a time both ways
  Switch Interface  one socket at a time (6), using ONE push button on a mono plug: plug it in, press and hold, let go, unplug
                  (with --per-contact: close each of the 12 contacts instead, for a stereo breakout)
  Timing          press two keys at once, 5 times, on the Jog & Shuttle; then the plugged-in button and a Jog & Shuttle key at once
  Lights          I switch each light myself and ask what you see (y/n, or key numbers): indicator LEDs (steady, flashing, faster,
                  off), the Jog & Shuttle's key backlights in both banks, brightness, and the Switch Interface's two electrical
                  outputs (optional). Only the documented temporary light commands are used; nothing is stored in a device, and every
                  light is switched back off at the end. Takes about 6 minutes (Jog & Shuttle about 4, Switch Interface about 1.5).
  Hot-plug        unplug each device when asked, then plug it back in

While it waits, it prints a line for every change in the device's report: the byte and bit that changed. Byte numbers count from 0
and byte 0 is the report ID. Each step waits 30 seconds, says what it is waiting for every 10 seconds, and then marks the step failed
and goes on. It never waits forever. After a result it pauses a few seconds before the next step.

Type a word and press Enter at any time (anything else you type is ignored, so keys that also type letters do no harm):
  skip     skip this step
  repeat   start this step again (also accepted for a few seconds after its result)
  quit     stop now; the steps not done are recorded as skipped

Before you start: close any other program that reads these devices (for example the TriggerGate service), or it takes some of the
reports. Except in the lights part, the guide only ever sends each device the request \"send me your current state\" (Generate Data,
command 177). The lights part additionally sends the documented temporary light commands (179, 180, 181, 182, 184, 186, 187) and
nothing that is stored in the device: never the unit ID, PID, save-backlight or reboot commands.

Options:
  --timeout S      seconds each step waits for you (default 30)
  --reps N         rounds in each timing step (default 5)
  --only LIST      run only some parts: keys, jog, inputs, timing, lights, hotplug (comma separated; default all)
                   e.g. --only lights  (the lights part only)
                   e.g. --only jog  (jog and shuttle only)   or   --only inputs --pid 0x04AB  (Switch Interface sockets only)
  --per-contact    test the Switch Interface contact by contact (a TRS breakout) instead of socket by socket
  --pid 0x0426     only the device with this product id
  --out DIR        where to write the result file (default: the current folder)
  --script FILE    take skip/repeat/quit from a file instead of the keyboard; lines: step-id [attempt] skip|repeat|quit
  --dry-run        no person: use the real devices, wait 2 s per step (change with --timeout), skip hot-plug. Checks that the tool
                   starts, finds the devices, and times out cleanly; every step is expected to fail.
  --simulate       no person and no devices: two simulated devices that follow the documentation, to check the tool itself.
  --help           this text
";

    /// The real devices, opened through the same exports the managed binding uses.
    struct RealHub {
        descs: Vec<DeviceDesc>,
        handles: Vec<*mut Device>,
        output_lens: Vec<usize>,
        t0: i64,
        frequency: i64,
        watch: *mut Watch,
        buffer: Vec<ReportEntry>,
    }

    fn describe(info: &DeviceInfo) -> DeviceDesc {
        DeviceDesc {
            pid: info.pid,
            product: wide_to_string(&info.product),
            version: info.version,
            input_len: info.input_len,
            output_len: info.output_len,
            timestamp_offset: xkhid_timestamp_offset(info.pid),
            path: wide_to_string(&info.path),
        }
    }

    fn open_info(info: &DeviceInfo) -> Result<*mut Device, String> {
        let mut device = core::ptr::null_mut();
        // SAFETY: the path is NUL-terminated; `device` is a valid out pointer.
        let result = unsafe { xkhid_open(info.path.as_ptr(), 4096, 0, xkhid_timestamp_offset(info.pid), &mut device) };
        if result == XKHID_OK { Ok(device) } else { Err(format!("xkhid_open error {result}")) }
    }

    fn consumer_interfaces() -> Vec<DeviceInfo> {
        enumerate(PI_ENGINEERING_VID, 0x000C, 0).into_iter().filter(|d| d.input_len > 0 && d.output_len > 0).collect()
    }

    impl RealHub {
        fn new(infos: &[DeviceInfo]) -> Result<RealHub, String> {
            let mut hub = RealHub {
                descs: infos.iter().map(describe).collect(),
                handles: Vec::new(),
                output_lens: infos.iter().map(|i| i.output_len as usize).collect(),
                t0: win32::qpc(),
                frequency: qpc_frequency(),
                watch: xkhid_watch_create(0),
                buffer: vec![ReportEntry::ZERO; 256],
            };
            for info in infos {
                // On failure `hub` is dropped, which closes what was opened.
                let h = open_info(info).map_err(|e| format!("could not open the {}: {e}", wide_to_string(&info.product)))?;
                hub.handles.push(h);
            }
            Ok(hub)
        }
    }

    impl Drop for RealHub {
        fn drop(&mut self) {
            for h in self.handles.drain(..) {
                // SAFETY: each handle came from `xkhid_open` and is closed once.
                unsafe { xkhid_close(h) };
            }
            // SAFETY: `watch` is null or from `xkhid_watch_create`; nothing uses it afterwards.
            unsafe { xkhid_watch_destroy(self.watch) };
        }
    }

    impl Hub for RealHub {
        fn devices(&self) -> &[DeviceDesc] {
            &self.descs
        }

        fn to_us(&self, qpc: i64) -> i64 {
            ((qpc - self.t0) as i128 * 1_000_000 / self.frequency as i128) as i64
        }

        fn now_us(&self) -> i64 {
            self.to_us(win32::qpc())
        }

        fn poll(&mut self, max_wait_us: i64) -> Vec<(usize, ReportEntry)> {
            std::thread::sleep(Duration::from_micros(max_wait_us.clamp(0, 5000) as u64));
            let mut out = Vec::new();
            for i in 0..self.handles.len() {
                if self.handles[i].is_null() {
                    continue;
                }
                // SAFETY: the handle is live; `buffer` holds 256 entries; this is the only reader.
                let count = unsafe { xkhid_read(self.handles[i], self.buffer.as_mut_ptr(), self.buffer.len() as i32) } as usize;
                let mut ended = false;
                for e in &self.buffer[..count] {
                    ended |= e.flags & FLAG_ERROR != 0;
                    out.push((i, *e));
                }
                if ended {
                    // The session is over (the device went away); free it. `reopen` makes a new one.
                    // SAFETY: the handle is live and not used again.
                    unsafe { xkhid_close(self.handles[i]) };
                    self.handles[i] = core::ptr::null_mut();
                }
            }
            out
        }

        fn request_report(&mut self, device: usize) -> Result<(), String> {
            let handle = self.handles[device];
            if handle.is_null() {
                return Err("not open".to_string());
            }
            let mut report = vec![0u8; self.output_lens[device]];
            report[1] = GENERATE_DATA;
            let mut win32_error = 0u32;
            // SAFETY: `handle` is live; `report` is output_len bytes.
            let result = unsafe { xkhid_write(handle, report.as_ptr(), report.len() as u32, 1000, &mut win32_error) };
            if result == XKHID_OK { Ok(()) } else { Err(format!("error {result}, Windows error {win32_error}")) }
        }

        fn is_open(&self, device: usize) -> bool {
            !self.handles[device].is_null()
        }

        fn reopen(&mut self, device: usize) -> Result<(), String> {
            if !self.handles[device].is_null() {
                return Ok(());
            }
            let taken: Vec<String> = (0..self.handles.len()).filter(|&i| !self.handles[i].is_null()).map(|i| self.descs[i].path.clone()).collect();
            let pid = self.descs[device].pid;
            let info = consumer_interfaces()
                .into_iter()
                .find(|i| i.pid == pid && !taken.contains(&wide_to_string(&i.path)))
                .ok_or_else(|| "not found among the connected devices".to_string())?;
            self.handles[device] = open_info(&info)?;
            self.descs[device] = describe(&info);
            Ok(())
        }

        fn watch_generation(&self) -> u64 {
            // SAFETY: `watch` is live until dropped.
            unsafe { xkhid_watch_generation(self.watch) }
        }

        /// Only volatile light reports pass `validate_light_report`; anything else (EEPROM writers, unit ID, PID, reboot, ...) is
        /// refused here even if a caller asked for it.
        fn send_light(&mut self, device: usize, report: &[u8]) -> Result<(), String> {
            let model = if self.descs[device].model() == guide::Model::JogShuttle { LightModel::JogShuttle } else { LightModel::SwitchInterface };
            validate_light_report(model, report)?;
            let handle = self.handles[device];
            if handle.is_null() {
                return Err("not open".to_string());
            }
            if report.len() != self.output_lens[device] {
                return Err(format!("report is {} bytes, the device wants {}", report.len(), self.output_lens[device]));
            }
            let mut win32_error = 0u32;
            // SAFETY: `handle` is live; `report` is output_len bytes.
            let result = unsafe { xkhid_write(handle, report.as_ptr(), report.len() as u32, 1000, &mut win32_error) };
            if result == XKHID_OK { Ok(()) } else { Err(format!("error {result}, Windows error {win32_error}")) }
        }
    }

    /// What the person typed: a command word, or any other line (an answer such as `y` or `1 2 3 4`).
    enum Line {
        Command(Command),
        Text(String),
    }

    /// Lines typed on the keyboard, read on a thread so waiting for the devices never blocks on the console.
    struct StdinInput {
        rx: std::sync::mpsc::Receiver<Line>,
        commands: std::collections::VecDeque<Command>,
        texts: std::collections::VecDeque<String>,
    }

    impl StdinInput {
        fn new() -> StdinInput {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdin.lock().read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let item = match guide::parse_command(&line) {
                                Some(c) => Line::Command(c),
                                None => Line::Text(line.trim().to_string()),
                            };
                            if tx.send(item).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            StdinInput { rx, commands: Default::default(), texts: Default::default() }
        }

        fn pull(&mut self) {
            while let Ok(item) = self.rx.try_recv() {
                match item {
                    Line::Command(c) => self.commands.push_back(c),
                    Line::Text(t) => self.texts.push_back(t),
                }
            }
        }
    }

    impl Input for StdinInput {
        fn poll(&mut self, _step: &str, _attempt: u32) -> Option<Command> {
            self.pull();
            self.commands.pop_front()
        }

        fn answer(&mut self, _step: &str, _attempt: u32) -> Option<String> {
            self.pull();
            self.texts.pop_front()
        }

        fn discard_pending(&mut self) {
            self.pull();
            self.texts.clear();
        }
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
    }

    fn write_result(path: &std::path::Path, meta: &guide::Meta, devices: &[DeviceDesc], outcome: &SessionOutcome) {
        let text = guide::result_json(meta, devices, outcome).render();
        // Write to a temporary name first so an interruption never leaves half a file under the real name.
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)) {
            eprintln!("warning: could not write {}: {e}", path.display());
        }
    }

    pub fn run(args: &Args) {
        if args.flag("--help") || args.flag("-h") || args.flag("help") {
            print!("{HELP}");
            return;
        }
        let simulate = args.flag("--simulate");
        let dry_run = args.flag("--dry-run");
        let mut sel = Selection::parse(args.value("--only").unwrap_or("all")).unwrap_or_else(|e| {
            eprintln!("--only: {e}");
            std::process::exit(2)
        });
        sel.per_contact |= args.flag("--per-contact");
        let reps = args.number("--reps", 5).clamp(1, 50) as u32;
        let default_timeout = if simulate { 5 } else if dry_run { 2 } else { 30 };
        let timeout_us = args.number("--timeout", default_timeout).max(1) * 1_000_000;
        let only_pid = args.number("--pid", -1);

        let mut options = if simulate || dry_run { Options::unattended(timeout_us) } else { Options::interactive() };
        options.timeout_us = timeout_us;
        options.skip_hotplug = dry_run;

        let mut hub: Box<dyn Hub> = if simulate {
            Box::new(SimHub::sdk_pair())
        } else {
            let infos: Vec<DeviceInfo> = consumer_interfaces().into_iter().filter(|d| only_pid < 0 || i64::from(d.pid) == only_pid).collect();
            if infos.is_empty() {
                println!("No X-keys device with an input and an output report was found. Plug the devices in, close any program that");
                println!("holds them open, and try again (`xkprobe list` shows what Windows sees).");
                std::process::exit(1);
            }
            match RealHub::new(&infos) {
                Ok(h) => Box::new(h),
                Err(e) => {
                    println!("{e}. Another program may have the device open; close it and try again.");
                    std::process::exit(1);
                }
            }
        };
        let devices = hub.devices().to_vec();
        let steps = guide::plan_steps(&devices, &sel, reps);
        if steps.is_empty() {
            println!("Nothing to do for these devices with --only {}.", args.value("--only").unwrap_or("all"));
            return;
        }
        let mut input: Box<dyn Input> = match args.value("--script") {
            Some(file) => match std::fs::read_to_string(file).map_err(|e| e.to_string()).and_then(|t| ScriptedInput::parse(&t)) {
                Ok(s) => Box::new(s),
                Err(e) => {
                    eprintln!("--script {file}: {e}");
                    std::process::exit(2);
                }
            },
            None if simulate || dry_run => Box::new(ScriptedInput::default()),
            None => Box::new(StdinInput::new()),
        };

        let started = unix_now();
        let out_dir = std::path::PathBuf::from(args.value("--out").unwrap_or("."));
        let file = out_dir.join(guide::result_file_name(started));
        let mode = if simulate { "simulate" } else if dry_run { "dry-run" } else { "interactive" };
        let meta = |finished: Option<i64>| guide::Meta {
            mode: mode.to_string(),
            started_utc: guide::iso_utc(started),
            finished_utc: finished.map(guide::iso_utc),
            qpc_frequency: qpc_frequency(),
            tool_version: format!("xkhid {}", env!("CARGO_PKG_VERSION")),
        };

        let labels = guide::device_labels(&devices);
        println!("X-keys guided test");
        println!("==================");
        println!("Devices found ({}):", devices.len());
        for (i, l) in labels.iter().enumerate() {
            println!("  {}. {l}", i + 1);
        }
        println!("{} steps. Each one tells you which device and what to do, then waits up to {} s.", steps.len(), timeout_us / 1_000_000);
        if !(simulate || dry_run) {
            println!("Type a word and press Enter at any time:  skip  (skip this step),  repeat  (do it again),  quit  (stop and save).");
            println!("Anything else you type is ignored. Byte numbers count from 0; byte 0 is the report ID.");
            println!("Apart from the lights part, only the request \"send me your current state\" is sent to a device. The lights part uses temporary light commands only; nothing is stored in a device.");
            println!("Close other programs that read these devices (such as the TriggerGate service) first.");
        } else {
            println!("Mode: {mode}; no person is involved.");
        }
        println!("Results are saved to {} after every step.", file.display());
        println!();

        let mut progress = |results: &[StepResult], counting_from: &str| {
            let partial = SessionOutcome { results: results.to_vec(), not_started: false, stopped_early: false, counting_from: counting_from.to_string() };
            write_result(&file, &meta(None), &devices, &partial);
        };
        let mut stdout = std::io::stdout();
        let outcome = guide::run_session(hub.as_mut(), input.as_mut(), &mut stdout, &options, &steps, &mut progress);
        if outcome.not_started {
            return;
        }
        write_result(&file, &meta(Some(unix_now())), &devices, &outcome);

        let count = |o: guide::Outcome| outcome.results.iter().filter(|r| r.outcome() == o).count();
        println!();
        println!("============================================================");
        println!(
            "Finished: {} passed, {} failed, {} skipped, of {} steps.{}",
            count(guide::Outcome::Pass),
            count(guide::Outcome::Fail),
            count(guide::Outcome::Skipped),
            outcome.results.len(),
            if outcome.stopped_early { " (stopped early)" } else { "" }
        );
        for r in outcome.results.iter().filter(|r| r.outcome() != guide::Outcome::Pass) {
            println!("  {:<7} step {:>2} {} - {}", r.outcome().as_str(), r.step.number, r.step.title, r.attempts.last().map_or("", |a| a.note.as_str()));
        }
        println!("Result file: {}", file.display());
    }
}

fn main() {
    let args = Args(std::env::args().skip(1).collect());
    match args.0.first().map(String::as_str) {
        Some("list") => list(),
        Some("probe") => probe(&args),
        Some("leak") => leak(&args),
        Some("watch") => watch(&args),
        Some("guide") => guided::run(&args),
        _ => {
            println!("usage: xkprobe list | probe [--requests N] [--gap-ms G] [--seconds S] [--pid P] [--mmcss] [--offset K] [--show-be a,b]");
            println!("               | leak [--iterations N] [--vid V] [--usage-page P] [--output-len L] | watch [--seconds S]");
            println!("               | guide [--help] [--timeout S] [--only LIST] [--pid P] [--out DIR] [--dry-run] [--simulate]");
        }
    }
}
