// Copyright (c) Barjonas LLC 2026.
//! `xkhid.dll`: X-keys (P.I. Engineering) HID access in native code, replacing `PIEHidNetCore.dll`.
//!
//! - Enumeration ([`xkhid_enumerate`]) fills a caller-supplied array and releases everything it allocates or opens before it
//!   returns; the vendor library leaked memory on every call.
//! - Each open device ([`xkhid_open`]) has one dedicated native read thread at time-critical priority that keeps an overlapped
//!   `ReadFile` outstanding and reads `QueryPerformanceCounter` the moment a read completes, before anything else. The report is
//!   copied into that device's single-producer, single-consumer ring ([`ReportEntry`]) and an auto-reset event is signaled. A
//!   read failure (for example an unplug) ends the session with one error entry, which always fits in the ring. No managed code
//!   runs on that thread, so a .NET garbage collection cannot delay a stamp. QPC ticks are the clock of `Stopwatch.GetTimestamp()`.
//! - Output reports are written synchronously with a timeout ([`xkhid_write`]).
//! - Arrival and removal of the vendor's HID interfaces are reported without a window ([`xkhid_watch_create`]) as an event plus a
//!   generation counter.
//!
//! Contract:
//! - Ring full: the newest report is dropped and counted ([`xkhid_dropped`]); every entry carries a sequence number, so the
//!   consumer sees where. The read thread never blocks on the consumer. The usual recovery is to send Generate Data (177) and
//!   take the state from the answer.
//! - One consumer: [`xkhid_read`] must never run on two threads at once for the same device.
//! - Lifetime: [`xkhid_close`] cancels the read, joins the thread and frees the device; no other export may be running or called
//!   for that device afterwards. [`xkhid_write`] calls may overlap each other but not [`xkhid_close`]. The handles returned by
//!   [`xkhid_event_handle`] and [`xkhid_watch_event_handle`] are the caller's to close and stay valid after close/destroy.
//! - The layouts of [`ReportEntry`] and [`DeviceInfo`] are mirrored in C#. Any change to them, to an export's
//!   signature or to the meaning of a flag or error code must change [`ABI_VERSION`] and the C# side together.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(windows, target_env = "msvc", not(target_feature = "crt-static")))]
compile_error!(
    "xkhid must link the C runtime statically (no Visual C++ redistributable on the target). Build from this crate's directory so .cargo/config.toml applies, and do not override RUSTFLAGS."
);

pub mod guide;
pub mod lights;
pub mod report;
pub mod ring;

#[cfg(windows)]
pub mod win32;
#[cfg(windows)]
mod enumerate;
#[cfg(windows)]
mod device;
#[cfg(windows)]
mod watch;

pub use report::*;

/// Changes whenever [`ReportEntry`], [`DeviceInfo`], an export's signature, or the meaning of a flag or error code changes. The C#
/// side refuses a DLL whose version differs.
pub const ABI_VERSION: u32 = 1;

/// [`ABI_VERSION`]; the caller checks it before using any other export.
#[unsafe(no_mangle)]
pub extern "system" fn xkhid_abi_version() -> u32 {
    ABI_VERSION
}

/// `size_of::<ReportEntry>()` (96), so the caller can check its mirror of the layout.
#[unsafe(no_mangle)]
pub extern "system" fn xkhid_entry_size() -> u32 {
    size_of::<ReportEntry>() as u32
}

/// `size_of::<DeviceInfo>()` (1296), so the caller can check its mirror of the layout.
#[unsafe(no_mangle)]
pub extern "system" fn xkhid_info_size() -> u32 {
    size_of::<DeviceInfo>() as u32
}

/// Where this model's 4-byte device timestamp sits in its input report, or -1 if unknown; pass it to [`xkhid_open`]. See
/// [`timestamp_offset_for_pid`].
#[unsafe(no_mangle)]
pub extern "system" fn xkhid_timestamp_offset(pid: u16) -> i32 {
    timestamp_offset_for_pid(pid)
}

fn vendor_or_default(vid: u16) -> u16 {
    if vid == 0 { PI_ENGINEERING_VID } else { vid }
}

#[cfg(windows)]
pub use exports::*;

#[cfg(windows)]
mod exports {
    use super::*;
    pub use crate::device::Device;
    pub use crate::watch::Watch;
    use crate::win32::Handle;

    /// Find the present HID interfaces of vendor `vid` (0 means P.I. Engineering, 0x05F3) whose top-level collection has usage page
    /// `usage_page` and output report length `output_len` (0 in either means any; today's consumers use 0x0C and 36), and store
    /// up to `max` of them in `out`. `*count` receives the number that matched. Interfaces with no input report are included
    /// (their `input_len` is 0). Allocates nothing that outlives the call, opens matching interfaces with no access rights and
    /// never opens other vendors' interfaces.
    ///
    /// Returns `XKHID_OK`, `XKHID_E_BUFFER_TOO_SMALL` (more matched than `max`; the first `max` are stored),
    /// `XKHID_E_BAD_ARGUMENT` or `XKHID_E_ENUMERATION_FAILED`.
    ///
    /// # Safety
    /// `out` must be valid for writing `max` entries (null is allowed when `max` is 0), and `count` for one `u32`.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_enumerate(
        vid: u16,
        usage_page: u16,
        output_len: u16,
        out: *mut DeviceInfo,
        max: u32,
        count: *mut u32,
    ) -> i32 {
        if count.is_null() || (out.is_null() && max > 0) {
            return XKHID_E_BAD_ARGUMENT;
        }
        // SAFETY: forwarded from the caller's promises.
        unsafe {
            let (result, matched) = crate::enumerate::enumerate(vendor_or_default(vid), usage_page, output_len, out, max);
            *count = matched;
            result
        }
    }

    /// Open the HID interface at `path` (from [`xkhid_enumerate`]) and start its read thread.
    ///
    /// - `ring_capacity`: entries; 0 means 256, otherwise clamped to 16..=4096 and rounded up to a power of two. One slot is kept
    ///   for the session-ending error entry.
    /// - `flags`: `OPEN_SUPPRESS_DUPLICATES` (1), `OPEN_MMCSS` (2).
    /// - `timestamp_offset`: index of the 4-byte big-endian device timestamp in the input report, decoded into
    ///   `ReportEntry::device_ms`; negative for none. [`xkhid_timestamp_offset`] gives the known value for a PID.
    ///
    /// Returns `XKHID_OK` with the device in `*device`, or an error with null there: `XKHID_E_BAD_ARGUMENT`, `XKHID_E_NOT_FOUND`,
    /// `XKHID_E_OPEN_READ_FAILED`, `XKHID_E_OPEN_WRITE_FAILED`, `XKHID_E_NO_INPUT_REPORT`, `XKHID_E_REPORT_TOO_LONG`,
    /// `XKHID_E_OUT_OF_MEMORY`.
    ///
    /// # Safety
    /// `path` must be null or a NUL-terminated UTF-16 string, and `device` null or valid for writing one pointer.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_open(
        path: *const u16,
        ring_capacity: u32,
        flags: u32,
        timestamp_offset: i32,
        device: *mut *mut Device,
    ) -> i32 {
        if path.is_null() || device.is_null() {
            return XKHID_E_BAD_ARGUMENT;
        }
        // SAFETY: forwarded from the caller's promises.
        unsafe {
            *device = core::ptr::null_mut();
            match Device::open(path, ring_capacity, flags, timestamp_offset) {
                Ok(opened) => {
                    *device = Box::into_raw(opened);
                    XKHID_OK
                }
                Err(error) => error,
            }
        }
    }

    /// Cancel the device's read, wait for its thread to finish, close its handles and free it. Null is ignored.
    ///
    /// # Safety
    /// `device` must be null or come from [`xkhid_open`] and not have been closed; no other export may be running for it.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_close(device: *mut Device) {
        if !device.is_null() {
            // SAFETY: the caller promised `device` is live and unused elsewhere.
            unsafe { Box::from_raw(device) }.close();
        }
    }

    /// A new handle to the device's auto-reset event, which is signaled after every entry. The caller owns it (`CloseHandle`);
    /// it stays valid after [`xkhid_close`]. Null on failure.
    ///
    /// # Safety
    /// `device` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_event_handle(device: *const Device) -> Handle {
        if device.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: the caller promised `device` is live.
        unsafe { (*device).event_handle() }
    }

    /// Move up to `max` queued entries, oldest first, into `entries`, and return how many were moved.
    ///
    /// # Safety
    /// `device` must be null or live, `entries` valid for writing `max` entries, and calls for one device must not overlap.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_read(device: *const Device, entries: *mut ReportEntry, max: i32) -> i32 {
        if device.is_null() || entries.is_null() || max <= 0 {
            return 0;
        }
        // SAFETY: forwarded from the caller's promises; at most `max` (an i32) entries are moved, so the count fits.
        unsafe { (*device).read(entries, max as usize) as i32 }
    }

    /// Reports dropped because the device's ring was full, since it was opened.
    ///
    /// # Safety
    /// `device` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_dropped(device: *const Device) -> u64 {
        if device.is_null() {
            return 0;
        }
        // SAFETY: the caller promised `device` is live.
        unsafe { (*device).dropped() }
    }

    /// Write one output report (`len` must equal the interface's output report length, report ID included) and wait up to
    /// `timeout_ms` (0xFFFFFFFF: forever) for it to complete; a write still pending then is cancelled. `*win32_error`, if not
    /// null, receives the Win32 error behind a failure, else 0. Returns `XKHID_OK`, `XKHID_E_BAD_ARGUMENT`,
    /// `XKHID_E_WRITE_LENGTH`, `XKHID_E_WRITE_TIMEOUT`, `XKHID_E_DEVICE_REMOVED`, `XKHID_E_WRITE_FAILED` or
    /// `XKHID_E_OUT_OF_MEMORY`. There is no sticky error: each call stands alone.
    ///
    /// # Safety
    /// `device` must be null or live, `data` readable for `len` bytes, `win32_error` null or writable.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_write(
        device: *const Device,
        data: *const u8,
        len: u32,
        timeout_ms: u32,
        win32_error: *mut u32,
    ) -> i32 {
        if device.is_null() || data.is_null() {
            return XKHID_E_BAD_ARGUMENT;
        }
        // SAFETY: forwarded from the caller's promises.
        unsafe {
            let (result, error) = (*device).write(core::slice::from_raw_parts(data, len as usize), timeout_ms);
            if !win32_error.is_null() {
                *win32_error = error;
            }
            result
        }
    }

    /// The device's input report length in bytes (report ID included), or -1 for null.
    ///
    /// # Safety
    /// `device` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_input_len(device: *const Device) -> i32 {
        // SAFETY: the caller promised `device` is null or live.
        unsafe { device.as_ref() }.map_or(-1, |d| i32::from(d.input_len()))
    }

    /// The device's output report length in bytes (report ID included; 0 if it has none), or -1 for null.
    ///
    /// # Safety
    /// `device` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_output_len(device: *const Device) -> i32 {
        // SAFETY: the caller promised `device` is null or live.
        unsafe { device.as_ref() }.map_or(-1, |d| i32::from(d.output_len()))
    }

    /// Start watching for HID interfaces of vendor `vid` (0 means P.I. Engineering) arriving or leaving. Null on failure. Free it
    /// with [`xkhid_watch_destroy`].
    #[unsafe(no_mangle)]
    pub extern "system" fn xkhid_watch_create(vid: u16) -> *mut Watch {
        Watch::create(vendor_or_default(vid)).map_or(core::ptr::null_mut(), Box::into_raw)
    }

    /// A new handle to the watch's auto-reset event, signaled after every arrival or removal it counts. The caller owns it; it
    /// stays valid after [`xkhid_watch_destroy`]. Null on failure.
    ///
    /// # Safety
    /// `watch` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_watch_event_handle(watch: *const Watch) -> Handle {
        // SAFETY: the caller promised `watch` is null or live.
        unsafe { watch.as_ref() }.map_or(core::ptr::null_mut(), Watch::event_handle)
    }

    /// Arrivals plus removals of the vendor's HID interfaces counted since the watch was made. When it differs from the value
    /// seen at the last enumeration, enumerate again.
    ///
    /// # Safety
    /// `watch` must be null or live.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_watch_generation(watch: *const Watch) -> u64 {
        // SAFETY: the caller promised `watch` is null or live.
        unsafe { watch.as_ref() }.map_or(0, Watch::generation)
    }

    /// Stop watching (waits for a notification callback still running) and free the watch. Null is ignored.
    ///
    /// # Safety
    /// `watch` must be null or come from [`xkhid_watch_create`] and not have been destroyed; no other export may be running for it.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn xkhid_watch_destroy(watch: *mut Watch) {
        if !watch.is_null() {
            // SAFETY: the caller promised `watch` is live and unused elsewhere.
            drop(unsafe { Box::from_raw(watch) });
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::win32::{CloseHandle, WAIT_TIMEOUT, WaitForSingleObject};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    #[test]
    fn version_and_sizes_are_exported() {
        assert_eq!(xkhid_abi_version(), ABI_VERSION);
        assert_eq!(xkhid_entry_size(), 96);
        assert_eq!(xkhid_info_size(), 1296);
        assert_eq!(xkhid_timestamp_offset(0x04AB), 32);
        assert_eq!(xkhid_timestamp_offset(0x0001), -1);
    }

    #[test]
    fn null_arguments_are_refused_or_ignored() {
        // SAFETY: every export accepts null for the pointers passed here.
        unsafe {
            let mut count = 0u32;
            assert_eq!(xkhid_enumerate(0, 0, 0, core::ptr::null_mut(), 1, &mut count), XKHID_E_BAD_ARGUMENT);
            assert_eq!(xkhid_enumerate(0, 0, 0, core::ptr::null_mut(), 0, core::ptr::null_mut()), XKHID_E_BAD_ARGUMENT);
            let mut device = core::ptr::dangling_mut::<Device>();
            assert_eq!(xkhid_open(core::ptr::null(), 0, 0, -1, &mut device), XKHID_E_BAD_ARGUMENT);
            xkhid_close(core::ptr::null_mut());
            assert!(xkhid_event_handle(core::ptr::null()).is_null());
            assert_eq!(xkhid_read(core::ptr::null(), core::ptr::null_mut(), 1), 0);
            assert_eq!(xkhid_dropped(core::ptr::null()), 0);
            assert_eq!(xkhid_write(core::ptr::null(), [0u8].as_ptr(), 1, 0, core::ptr::null_mut()), XKHID_E_BAD_ARGUMENT);
            assert_eq!(xkhid_input_len(core::ptr::null()), -1);
            assert_eq!(xkhid_output_len(core::ptr::null()), -1);
            assert!(xkhid_watch_event_handle(core::ptr::null()).is_null());
            assert_eq!(xkhid_watch_generation(core::ptr::null()), 0);
            xkhid_watch_destroy(core::ptr::null_mut());
        }
    }

    #[test]
    fn opening_a_missing_path_fails_cleanly() {
        let path = wide(r"\\?\hid#vid_05f3&pid_ffff&mi_00#0&0&0&0#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        let mut device = core::ptr::dangling_mut::<Device>();
        // SAFETY: `path` is NUL-terminated; `device` is a valid out pointer.
        let result = unsafe { xkhid_open(path.as_ptr(), 0, 0, -1, &mut device) };
        assert_eq!(result, XKHID_E_NOT_FOUND);
        assert!(device.is_null());
        // SAFETY: as above.
        assert_eq!(unsafe { xkhid_open(path.as_ptr(), 0, 1 << 7, -1, &mut device) }, XKHID_E_BAD_ARGUMENT, "unknown flag");
    }

    #[test]
    fn enumerating_a_vendor_with_no_devices_finds_nothing_and_opens_nothing() {
        let mut out = [DeviceInfo::ZERO; 2];
        let mut count = 99u32;
        // 0xFFFF is no vendor's id, so no path matches and no interface is opened.
        // SAFETY: `out` holds 2 entries; `count` is writable.
        let result = unsafe { xkhid_enumerate(0xFFFF, 0, 0, out.as_mut_ptr(), 2, &mut count) };
        assert_eq!((result, count), (XKHID_OK, 0));
        assert_eq!(out[0], DeviceInfo::ZERO);
    }

    /// The P.I. Engineering consumer-control interfaces present, for the hardware tests.
    fn consumer_interfaces() -> Vec<DeviceInfo> {
        let mut out = [DeviceInfo::ZERO; 8];
        let mut count = 0u32;
        // SAFETY: `out` holds 8 entries; `count` is writable.
        unsafe { xkhid_enumerate(0, 0x0C, 36, out.as_mut_ptr(), 8, &mut count) };
        out[..(count as usize).min(8)].iter().copied().filter(|d| d.input_len > 0).collect()
    }

    /// Generate Data (P.I. SDK command 177): the device answers with one input report. The only output report these tests send.
    fn generate_data(device: *const Device) -> i32 {
        let mut report = [0u8; 36];
        report[1] = 177;
        let mut win32_error = 0;
        // SAFETY: `device` is live; `report` is 36 bytes, the consumer interface's output length.
        unsafe { xkhid_write(device, report.as_ptr(), 36, 1000, &mut win32_error) }
    }

    fn open(info: &DeviceInfo, capacity: u32, flags: u32) -> *mut Device {
        let mut device = core::ptr::null_mut();
        // SAFETY: the path is NUL-terminated; `device` is a valid out pointer.
        let result = unsafe { xkhid_open(info.path.as_ptr(), capacity, flags, xkhid_timestamp_offset(info.pid), &mut device) };
        assert_eq!(result, XKHID_OK, "open PID {:04X}", info.pid);
        device
    }

    fn read_all(device: *const Device) -> Vec<ReportEntry> {
        let mut out = vec![ReportEntry::ZERO; 4096];
        // SAFETY: `device` is live; `out` holds 4096; one reader.
        let count = unsafe { xkhid_read(device, out.as_mut_ptr(), 4096) };
        out.truncate(count as usize);
        out
    }

    /// Needs a P.I. Engineering device plugged in: `cargo test -- --ignored`. Sends only Generate Data.
    #[test]
    #[ignore = "needs an X-keys device"]
    fn hardware_generate_data_is_answered_stamped_and_signaled() {
        let devices = consumer_interfaces();
        assert!(!devices.is_empty(), "no X-keys consumer-control interface plugged in");
        for info in devices {
            let device = open(&info, 0, 0);
            // SAFETY: `device` is live until closed; the event duplicate is closed here.
            unsafe {
                assert_eq!(xkhid_input_len(device), i32::from(info.input_len));
                assert_eq!(xkhid_output_len(device), 36);
                let event = xkhid_event_handle(device);
                let mut short = [0u8; 35];
                assert_eq!(xkhid_write(device, short.as_mut_ptr(), 35, 1000, core::ptr::null_mut()), XKHID_E_WRITE_LENGTH);
                let before = crate::win32::qpc();
                assert_eq!(generate_data(device), XKHID_OK);
                assert_eq!(WaitForSingleObject(event, 1000), crate::win32::WAIT_OBJECT_0, "no report within 1 s");
                std::thread::sleep(std::time::Duration::from_millis(50));
                let got = read_all(device);
                assert_eq!(got.len(), 1, "one answer, nothing else");
                let e = got[0];
                assert!(e.qpc > before);
                assert_eq!((e.sequence, e.error, i32::from(e.length)), (0, 0, i32::from(info.input_len)));
                assert_eq!(e.data[2] & 0x02, 0x02, "data type: generated");
                if xkhid_timestamp_offset(info.pid) >= 0 {
                    assert_ne!(e.flags & FLAG_HAS_DEVICE_MS, 0);
                }
                xkhid_close(device);
                assert_ne!(CloseHandle(event), 0);
            }
        }
    }

    /// A ring of 16 keeps 15 reports and counts the rest as dropped; sequence numbers still count every report.
    #[test]
    #[ignore = "needs an X-keys device"]
    fn hardware_a_full_ring_drops_and_counts() {
        let info = *consumer_interfaces().first().expect("no X-keys consumer-control interface plugged in");
        let device = open(&info, 16, 0);
        for _ in 0..20 {
            assert_eq!(generate_data(device), XKHID_OK);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let got = read_all(device);
        // SAFETY: `device` is live until closed.
        unsafe {
            assert_eq!(got.len(), 15);
            assert_eq!(xkhid_dropped(device), 5);
            assert_eq!(got.iter().map(|e| e.sequence).collect::<Vec<_>>(), (0..15).collect::<Vec<_>>());
            // After draining, the next report gets sequence 20: the gap shows the five drops.
            assert_eq!(generate_data(device), XKHID_OK);
            std::thread::sleep(std::time::Duration::from_millis(100));
            let next = read_all(device);
            assert_eq!(next.len(), 1);
            assert_eq!(next[0].sequence, 20);
            xkhid_close(device);
        }
    }

    /// Generated reports differ only in their device timestamp, so duplicate suppression keeps the first.
    #[test]
    #[ignore = "needs an X-keys device"]
    fn hardware_duplicate_suppression_ignores_the_timestamp() {
        let info = *consumer_interfaces().iter().find(|d| xkhid_timestamp_offset(d.pid) >= 0).expect("no timestamped X-keys");
        let device = open(&info, 0, OPEN_SUPPRESS_DUPLICATES);
        for _ in 0..3 {
            assert_eq!(generate_data(device), XKHID_OK);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(read_all(device).len(), 1);
        // SAFETY: `device` is live.
        unsafe { xkhid_close(device) };
    }

    /// Closing while the read thread has a read outstanding (the normal state) returns promptly, many times over.
    #[test]
    #[ignore = "needs an X-keys device"]
    fn hardware_open_and_close_repeatedly() {
        let info = *consumer_interfaces().first().expect("no X-keys consumer-control interface plugged in");
        let start = std::time::Instant::now();
        for _ in 0..200 {
            let device = open(&info, 0, OPEN_MMCSS);
            // SAFETY: `device` is live.
            unsafe { xkhid_close(device) };
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
    }

    #[test]
    fn a_watch_can_be_made_waited_on_and_destroyed() {
        let watch = xkhid_watch_create(0);
        assert!(!watch.is_null());
        // SAFETY: `watch` is live until destroyed; the duplicate is closed here.
        unsafe {
            assert_eq!(xkhid_watch_generation(watch), 0);
            let event = xkhid_watch_event_handle(watch);
            assert!(!event.is_null());
            assert_eq!(WaitForSingleObject(event, 0), WAIT_TIMEOUT);
            xkhid_watch_destroy(watch);
            assert_eq!(WaitForSingleObject(event, 0), WAIT_TIMEOUT, "the caller's duplicate outlives the watch");
            assert_ne!(CloseHandle(event), 0);
        }
    }
}
