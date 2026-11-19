// Copyright (c) Barjonas LLC 2026.
//! The ABI's data types and error codes, and the pure (platform-independent) logic around them: device-timestamp decoding,
//! duplicate comparison, the enumeration filter, and HID path parsing.

/// The longest input report a [`ReportEntry`] carries, report ID included. X-keys reports are 33 or 37 bytes.
pub const MAX_REPORT_LEN: usize = 64;

/// UTF-16 units in [`DeviceInfo::path`], terminating NUL included. A longer path is not reported.
pub const PATH_CHARS: usize = 512;

/// UTF-16 units in [`DeviceInfo::manufacturer`] and [`DeviceInfo::product`], terminating NUL included (longer strings are cut).
pub const STRING_CHARS: usize = 64;

/// P.I. Engineering's USB vendor id; what a vendor id of 0 means in [`crate::xkhid_enumerate`] and [`crate::xkhid_watch_create`].
pub const PI_ENGINEERING_VID: u16 = 0x05F3;

/// `ReportEntry::flags`: `device_ms` holds the device's own timestamp, decoded from the report.
pub const FLAG_HAS_DEVICE_MS: u32 = 1 << 0;
/// `ReportEntry::flags`: a synthetic entry that ends the session (`length` 0, `error` and `win32_error` say why). It is always the
/// last entry the device produces; the consumer should close the device and re-enumerate.
pub const FLAG_ERROR: u32 = 1 << 1;
/// `ReportEntry::flags`: the read completed at once, because the report was already waiting in the HID class driver's queue (it
/// arrived while no read was pending). Its `qpc` is when the read thread picked it up, which is later than its arrival by an
/// unknown amount; `device_ms`, where present, is the better guide for such an entry.
pub const FLAG_QUEUED: u32 = 1 << 2;

/// `xkhid_open` flags: drop a report whose bytes equal the previous report's, ignoring the device-timestamp bytes.
pub const OPEN_SUPPRESS_DUPLICATES: u32 = 1 << 0;
/// `xkhid_open` flags: register the read thread with MMCSS ("Pro Audio") as well as raising it to time-critical priority.
pub const OPEN_MMCSS: u32 = 1 << 1;
/// Every flag `xkhid_open` accepts.
pub const OPEN_ALL_FLAGS: u32 = OPEN_SUPPRESS_DUPLICATES | OPEN_MMCSS;

// Error codes returned by the exports and carried in `ReportEntry::error`.
pub const XKHID_OK: i32 = 0;
/// A null pointer, an unknown flag, or a value out of range.
pub const XKHID_E_BAD_ARGUMENT: i32 = 1;
/// The device path does not exist (any more).
pub const XKHID_E_NOT_FOUND: i32 = 2;
/// The path exists but could not be opened for reading (vendor 207).
pub const XKHID_E_OPEN_READ_FAILED: i32 = 3;
/// The path could be opened for reading but not for writing (vendor 208).
pub const XKHID_E_OPEN_WRITE_FAILED: i32 = 4;
/// Reserved (vendor 203). The crate does not track opens; opening one path twice gives two independent sessions.
pub const XKHID_E_ALREADY_OPEN: i32 = 5;
/// The device went away: unplugged, disabled, or its handle became invalid (vendor 308/309/408).
pub const XKHID_E_DEVICE_REMOVED: i32 = 6;
/// A read failed for another reason; `win32_error` carries it (vendor 310/320/321).
pub const XKHID_E_READ_FAILED: i32 = 7;
/// The interface has no input report (for example an output-only PID mode), so there is nothing to read (vendor 302/703).
pub const XKHID_E_NO_INPUT_REPORT: i32 = 8;
/// The write length is not the interface's output report length, or the interface has no output report (vendor 402/403).
pub const XKHID_E_WRITE_LENGTH: i32 = 9;
/// The write did not complete within the timeout and was cancelled (vendor 411).
pub const XKHID_E_WRITE_TIMEOUT: i32 = 10;
/// The write failed for another reason; the Win32 error says why (vendor 409/410/412).
pub const XKHID_E_WRITE_FAILED: i32 = 11;
/// More interfaces matched than the caller's array holds; the first `max` were stored and `*count` is the full number.
pub const XKHID_E_BUFFER_TOO_SMALL: i32 = 12;
/// An event, a thread or memory could not be created.
pub const XKHID_E_OUT_OF_MEMORY: i32 = 13;
/// The interface's input report is longer than [`MAX_REPORT_LEN`].
pub const XKHID_E_REPORT_TOO_LONG: i32 = 14;
/// Windows' device enumeration (`SetupDiGetClassDevsW`) failed.
pub const XKHID_E_ENUMERATION_FAILED: i32 = 15;

/// One input report as read and stamped by a device's read thread, or one session-ending error. 96 bytes, 8-byte aligned;
/// mirrored in C#.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportEntry {
    /// `QueryPerformanceCounter` ticks read by the device's read thread the moment its overlapped `ReadFile` completed, before any
    /// other work (the same clock as `Stopwatch.GetTimestamp()`). See [`FLAG_QUEUED`].
    pub qpc: i64,
    /// The device's millisecond counter (since plug-in), decoded big-endian from the report; valid only with [`FLAG_HAS_DEVICE_MS`].
    pub device_ms: u32,
    /// Counts every entry the device produced, including ones dropped because the ring was full; a gap means a drop.
    pub sequence: u32,
    /// `FLAG_*`.
    pub flags: u32,
    /// 0, or an `XKHID_E_*` code when `flags` has [`FLAG_ERROR`].
    pub error: i32,
    /// Valid bytes in `data`: the bytes the read returned (the interface's input report length); 0 for an error entry.
    pub length: u16,
    pub reserved: u16,
    /// The Win32 error of the failed read for an error entry, else 0.
    pub win32_error: u32,
    /// The report as read, index 0 = report ID; bytes from `length` on are zero.
    pub data: [u8; MAX_REPORT_LEN],
}

impl ReportEntry {
    pub const ZERO: ReportEntry = ReportEntry {
        qpc: 0,
        device_ms: 0,
        sequence: 0,
        flags: 0,
        error: 0,
        length: 0,
        reserved: 0,
        win32_error: 0,
        data: [0; MAX_REPORT_LEN],
    };

    /// The entry for one report. `report` longer than [`MAX_REPORT_LEN`] is cut (`xkhid_open` refuses such interfaces anyway).
    pub fn report(qpc: i64, sequence: u32, queued: bool, report: &[u8], timestamp_offset: i32) -> ReportEntry {
        let length = report.len().min(MAX_REPORT_LEN);
        let mut entry = ReportEntry { qpc, sequence, length: length as u16, ..ReportEntry::ZERO };
        entry.data[..length].copy_from_slice(&report[..length]);
        if queued {
            entry.flags |= FLAG_QUEUED;
        }
        if let Some(ms) = decode_device_ms(&report[..length], timestamp_offset) {
            entry.device_ms = ms;
            entry.flags |= FLAG_HAS_DEVICE_MS;
        }
        entry
    }

    /// The session-ending entry for a failed read.
    pub fn error(qpc: i64, sequence: u32, error: i32, win32_error: u32) -> ReportEntry {
        ReportEntry { qpc, sequence, flags: FLAG_ERROR, error, win32_error, ..ReportEntry::ZERO }
    }
}

/// One HID interface (one top-level collection) as found by `xkhid_enumerate`. 1296 bytes, 2-byte aligned; mirrored in C#.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The interface's device path, NUL-terminated; pass it to `xkhid_open`.
    pub path: [u16; PATH_CHARS],
    /// `HIDD_ATTRIBUTES`.
    pub vid: u16,
    pub pid: u16,
    pub version: u16,
    /// `HIDP_CAPS` of the interface's top-level collection.
    pub usage_page: u16,
    pub usage: u16,
    /// Report lengths in bytes including the report-ID byte; 0 means the interface has no report of that kind.
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    /// NUL-terminated; empty if the device does not answer.
    pub manufacturer: [u16; STRING_CHARS],
    pub product: [u16; STRING_CHARS],
}

impl DeviceInfo {
    pub const ZERO: DeviceInfo = DeviceInfo {
        path: [0; PATH_CHARS],
        vid: 0,
        pid: 0,
        version: 0,
        usage_page: 0,
        usage: 0,
        input_len: 0,
        output_len: 0,
        feature_len: 0,
        manufacturer: [0; STRING_CHARS],
        product: [0; STRING_CHARS],
    };
}

/// Where the 4-byte, big-endian, millisecond device timestamp sits in this model's input report (index 0 = report ID), or -1.
///
/// Measured on a test machine (`docs/device-measurements.md`, "Measured with `xkprobe probe`"): XK-12 Jog & Shuttle and XK-12 Switch Interface. From P.I.'s SDK
/// pages and not measured: XK-3 Switch Interface (same report as the XK-12 SI) and XK-12 Joystick. Other models: unknown.
pub fn timestamp_offset_for_pid(pid: u16) -> i32 {
    match pid {
        // XK-12 Jog & Shuttle, PID #1..#3 (#2 has no input report). Measured at 9..12 (SDK "bytes 10-13").
        0x0426..=0x0428 => 9,
        // XK-12 Switch Interface PID #1..#4 (measured), XK-3 Switch Interface PID #1..#4 (SDK). Bytes 33..36 of the SDK.
        0x04A8..=0x04AB | 0x04C5..=0x04C8 => 32,
        // XK-12 Joystick (SDK, not measured).
        0x0429..=0x042B => 13,
        _ => -1,
    }
}

/// The device timestamp at `offset` (4 bytes, most significant first), or `None` when `offset` is negative or the report is too
/// short. Four zero bytes also give `None`: that is what a device with its timestamp turned off sends, and a real count of 0
/// lasts only the first millisecond after plug-in.
pub fn decode_device_ms(report: &[u8], offset: i32) -> Option<u32> {
    let offset = usize::try_from(offset).ok()?;
    let bytes: [u8; 4] = report.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    let ms = u32::from_be_bytes(bytes);
    (ms != 0).then_some(ms)
}

/// Whether two reports are equal apart from the four device-timestamp bytes at `offset` (negative: compare every byte).
pub fn same_ignoring_timestamp(a: &[u8], b: &[u8], offset: i32) -> bool {
    if a.len() != b.len() {
        return false;
    }
    match usize::try_from(offset) {
        Ok(start) if start < a.len() => {
            let end = (start + 4).min(a.len());
            a[..start] == b[..start] && a[end..] == b[end..]
        }
        _ => a == b,
    }
}

/// The enumeration filter after the vendor id: 0 in a filter field means "any".
pub fn caps_match(usage_page_filter: u16, output_len_filter: u16, usage_page: u16, output_len: u16) -> bool {
    (usage_page_filter == 0 || usage_page_filter == usage_page) && (output_len_filter == 0 || output_len_filter == output_len)
}

/// Whether a HID device path names vendor `vid`, so enumeration can skip other vendors' interfaces without opening them.
/// Understands the USB form (`\\?\hid#vid_05f3&pid_0426&mi_00#...`) and the Bluetooth form (`..._vid&000205f3_pid&...`),
/// case-insensitively. A path in neither form is reported as not matching.
pub fn path_has_vid(path: &[u16], vid: u16) -> bool {
    let lower = |c: u16| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c };
    let hex = |s: &[u16]| -> Option<u32> {
        s.iter().try_fold(0u32, |acc, &c| {
            let digit = char::from_u32(u32::from(c))?.to_digit(16)?;
            Some(acc * 16 + digit)
        })
    };
    let n = path.len();
    let mut i = 0;
    while i + 4 <= n {
        if lower(path[i]) == u16::from(b'v') && lower(path[i + 1]) == u16::from(b'i') && lower(path[i + 2]) == u16::from(b'd') {
            let digits = &path[i + 4..];
            let found = match path[i + 3] {
                c if c == u16::from(b'_') => digits.get(..4).and_then(hex),
                c if c == u16::from(b'&') => digits.get(..8).and_then(hex).map(|v| v & 0xFFFF),
                _ => None,
            };
            if found == Some(u32::from(vid)) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Copy `src` (up to its first NUL) into `dst` with a terminating NUL. Returns false, leaving `dst` empty, if it does not fit.
pub fn copy_wide(dst: &mut [u16], src: &[u16]) -> bool {
    let len = src.iter().position(|&c| c == 0).unwrap_or(src.len());
    if dst.is_empty() || len >= dst.len() {
        if let Some(first) = dst.first_mut() {
            *first = 0;
        }
        return false;
    }
    dst[..len].copy_from_slice(&src[..len]);
    dst[len..].fill(0);
    true
}

// Win32 error codes the mapping below needs.
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_READY: u32 = 21;
pub const ERROR_BAD_COMMAND: u32 = 22;
pub const ERROR_GEN_FAILURE: u32 = 31;
pub const ERROR_OPERATION_ABORTED: u32 = 995;
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
pub const ERROR_NO_SUCH_DEVICE: u32 = 433;

/// Whether a failed read or write's Win32 error means the device is gone. A read on an unplugged HID device usually fails with
/// `ERROR_DEVICE_NOT_CONNECTED`; the others are what removal or a disabled device has been seen to give. Not yet observed on a
/// hot unplug with this crate (needs a person).
pub fn is_removal(win32_error: u32) -> bool {
    matches!(
        win32_error,
        ERROR_DEVICE_NOT_CONNECTED
            | ERROR_INVALID_HANDLE
            | ERROR_FILE_NOT_FOUND
            | ERROR_NOT_READY
            | ERROR_BAD_COMMAND
            | ERROR_GEN_FAILURE
            | ERROR_OPERATION_ABORTED
            | ERROR_NO_SUCH_DEVICE
    )
}

/// The `XKHID_E_*` code for a failed read.
pub fn read_error_code(win32_error: u32) -> i32 {
    if is_removal(win32_error) { XKHID_E_DEVICE_REMOVED } else { XKHID_E_READ_FAILED }
}

/// The `XKHID_E_*` code for a failed write.
pub fn write_error_code(win32_error: u32) -> i32 {
    if is_removal(win32_error) { XKHID_E_DEVICE_REMOVED } else { XKHID_E_WRITE_FAILED }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn report_entry_layout_matches_the_csharp_mirror() {
        assert_eq!(size_of::<ReportEntry>(), 96);
        assert_eq!(align_of::<ReportEntry>(), 8);
        assert_eq!(offset_of!(ReportEntry, qpc), 0);
        assert_eq!(offset_of!(ReportEntry, device_ms), 8);
        assert_eq!(offset_of!(ReportEntry, sequence), 12);
        assert_eq!(offset_of!(ReportEntry, flags), 16);
        assert_eq!(offset_of!(ReportEntry, error), 20);
        assert_eq!(offset_of!(ReportEntry, length), 24);
        assert_eq!(offset_of!(ReportEntry, reserved), 26);
        assert_eq!(offset_of!(ReportEntry, win32_error), 28);
        assert_eq!(offset_of!(ReportEntry, data), 32);
    }

    #[test]
    fn device_info_layout_matches_the_csharp_mirror() {
        assert_eq!(size_of::<DeviceInfo>(), 1296);
        assert_eq!(align_of::<DeviceInfo>(), 2);
        assert_eq!(offset_of!(DeviceInfo, path), 0);
        assert_eq!(offset_of!(DeviceInfo, vid), 1024);
        assert_eq!(offset_of!(DeviceInfo, pid), 1026);
        assert_eq!(offset_of!(DeviceInfo, version), 1028);
        assert_eq!(offset_of!(DeviceInfo, usage_page), 1030);
        assert_eq!(offset_of!(DeviceInfo, usage), 1032);
        assert_eq!(offset_of!(DeviceInfo, input_len), 1034);
        assert_eq!(offset_of!(DeviceInfo, output_len), 1036);
        assert_eq!(offset_of!(DeviceInfo, feature_len), 1038);
        assert_eq!(offset_of!(DeviceInfo, manufacturer), 1040);
        assert_eq!(offset_of!(DeviceInfo, product), 1168);
    }

    /// Reports recorded on a test machine with `xkprobe probe` after a Generate Data request (`docs/device-measurements.md`, "Measured with `xkprobe probe`").
    /// Jog & Shuttle: report ID 0, unit ID 2, data type 2 (generated, program switch unset), D3 bit 7 (shuttle at rest), device
    /// timestamp 0x000D1CBE = 859326 ms at indexes 9..12.
    const JOG_SHUTTLE_REPORT: [u8; 33] = [
        0x00, 0x02, 0x02, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x0D, 0x1C, 0xBE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    /// Switch Interface: report ID 0, unit ID 1, data type 2, device timestamp 0x000CE9BE = 846270 ms at indexes 32..35, reboot
    /// counter 1 at index 36.
    const SWITCH_INTERFACE_REPORT: [u8; 37] = [
        0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0xE9, 0xBE, 0x01,
    ];

    #[test]
    fn the_device_timestamp_is_decoded_big_endian_at_the_model_offset() {
        assert_eq!(decode_device_ms(&JOG_SHUTTLE_REPORT, timestamp_offset_for_pid(0x0426)), Some(0x000D_1CBE));
        assert_eq!(decode_device_ms(&SWITCH_INTERFACE_REPORT, timestamp_offset_for_pid(0x04AB)), Some(0x000C_E9BE));
        assert_eq!(decode_device_ms(&[0, 0, 0, 1, 2, 3, 4], 3), Some(0x0102_0304));
    }

    #[test]
    fn no_timestamp_without_an_offset_a_long_enough_report_or_a_non_zero_value() {
        assert_eq!(decode_device_ms(&JOG_SHUTTLE_REPORT, -1), None);
        assert_eq!(decode_device_ms(&JOG_SHUTTLE_REPORT, 30), None, "30..34 runs past a 33-byte report");
        assert_eq!(decode_device_ms(&JOG_SHUTTLE_REPORT, 10), Some(0x0D1C_BE00), "the SDK's other reading, 10..13, is 256 times too big");
        assert_eq!(decode_device_ms(&JOG_SHUTTLE_REPORT, 29), None, "29..33 is all zero");
        assert_eq!(decode_device_ms(&[1, 2, 3], 0), None);
        assert_eq!(decode_device_ms(&[], 0), None);
        assert_eq!(decode_device_ms(&[1, 2, 3, 4], i32::MAX), None);
        assert_eq!(timestamp_offset_for_pid(0x0405), -1, "XK-24: unknown");
    }

    #[test]
    fn duplicates_are_compared_without_the_timestamp_bytes() {
        let a = JOG_SHUTTLE_REPORT;
        let mut b = a;
        b[9..13].copy_from_slice(&[0xFF, 0xEE, 0xDD, 0xCC]);
        assert!(same_ignoring_timestamp(&a, &b, 9));
        assert!(!same_ignoring_timestamp(&a, &b, -1));
        b[3] = 1;
        assert!(!same_ignoring_timestamp(&a, &b, 9));
        assert!(!same_ignoring_timestamp(&a, &a[..32], 9), "lengths differ");
        // A timestamp at the very end, and an offset past the end, are handled.
        let mut c = SWITCH_INTERFACE_REPORT;
        c[35] ^= 0xFF;
        assert!(same_ignoring_timestamp(&SWITCH_INTERFACE_REPORT, &c, 32));
        assert!(!same_ignoring_timestamp(&SWITCH_INTERFACE_REPORT, &c, 100));
    }

    #[test]
    fn report_entries_carry_the_bytes_the_stamp_and_the_flags() {
        let e = ReportEntry::report(1234, 7, false, &JOG_SHUTTLE_REPORT, 9);
        assert_eq!((e.qpc, e.sequence, e.length, e.error, e.win32_error), (1234, 7, 33, 0, 0));
        assert_eq!(e.flags, FLAG_HAS_DEVICE_MS);
        assert_eq!(e.device_ms, 859_326);
        assert_eq!(&e.data[..33], &JOG_SHUTTLE_REPORT[..]);
        assert!(e.data[33..].iter().all(|&b| b == 0));

        let e = ReportEntry::report(1, 2, true, &[9; 100], -1);
        assert_eq!(e.flags, FLAG_QUEUED);
        assert_eq!(e.length as usize, MAX_REPORT_LEN);

        let e = ReportEntry::error(5, 8, XKHID_E_DEVICE_REMOVED, ERROR_DEVICE_NOT_CONNECTED);
        assert_eq!((e.flags, e.error, e.win32_error, e.length, e.sequence), (FLAG_ERROR, 6, 1167, 0, 8));
    }

    /// Capabilities recorded on a test machine (`docs/device-measurements.md`, "Device capabilities"): (pid, usage page, usage, input, output).
    const RECORDED: [(u16, u16, u16, u16, u16); 6] = [
        (0x0426, 0x000C, 0x0001, 33, 36),
        (0x0426, 0x0001, 0x0006, 9, 2),
        (0x0426, 0x0001, 0x0002, 6, 0),
        (0x04AB, 0x000C, 0x0001, 37, 36),
        (0x04AB, 0x0001, 0x0006, 9, 2),
        (0x04AB, 0x0001, 0x0002, 7, 0),
    ];

    #[test]
    fn todays_consumer_filter_picks_exactly_the_consumer_control_interfaces() {
        let picked: Vec<_> = RECORDED.iter().filter(|r| caps_match(0x0C, 36, r.1, r.4)).map(|r| (r.0, r.3)).collect();
        assert_eq!(picked, vec![(0x0426, 33), (0x04AB, 37)]);
        assert_eq!(RECORDED.iter().filter(|r| caps_match(0, 0, r.1, r.4)).count(), 6, "0 means any");
        assert_eq!(RECORDED.iter().filter(|r| caps_match(0x01, 0, r.1, r.4)).count(), 4);
        assert_eq!(RECORDED.iter().filter(|r| caps_match(0, 2, r.1, r.4)).count(), 2);
    }

    #[test]
    fn the_vendor_id_is_found_in_usb_and_bluetooth_paths() {
        // Paths as recorded on a test machine (the last one upper-cased, to check that case does not matter).
        let jog = w(r"\\?\hid#vid_05f3&pid_0426&mi_00#c&8a9fe31&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        let keyboard = w(r"\\?\hid#vid_05f3&pid_0426&mi_01#c&206d8a76&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}\kbd");
        let switch = w(r"\\?\HID#VID_05F3&PID_04AB&MI_00#B&1EDB37C8&0&0000#{4D1E55B2-F16F-11CF-88CB-001111000030}");
        assert!(path_has_vid(&jog, 0x05F3));
        assert!(path_has_vid(&keyboard, 0x05F3));
        assert!(path_has_vid(&switch, 0x05F3));
        assert!(!path_has_vid(&jog, 0x046D));
        let bluetooth = w(r"\\?\hid#{00001124-0000-1000-8000-00805f9b34fb}_vid&000205f3_pid&0001#9&1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}");
        assert!(path_has_vid(&bluetooth, 0x05F3));
        assert!(!path_has_vid(&w(r"\\?\hid#vid_05f#x"), 0x05F3), "too few digits");
        assert!(!path_has_vid(&w(r"\\?\hid#vid_zzzz&vid"), 0x05F3));
        assert!(!path_has_vid(&w(r"\\?\acpi#pnp0c50#1"), 0x05F3), "no vendor id: not matched");
        assert!(!path_has_vid(&[], 0x05F3));
    }

    #[test]
    fn wide_strings_are_copied_with_a_terminator_or_not_at_all() {
        let mut dst = [0xFFFFu16; 6];
        assert!(copy_wide(&mut dst, &w("abc")));
        assert_eq!(dst, [97, 98, 99, 0, 0, 0]);
        assert!(copy_wide(&mut dst, &[104, 105, 0, 120]), "stops at the first NUL");
        assert_eq!(&dst[..3], &[104, 105, 0]);
        assert!(copy_wide(&mut dst, &w("abcde")));
        assert!(!copy_wide(&mut dst, &w("abcdef")), "no room for the NUL");
        assert_eq!(dst[0], 0);
        assert!(!copy_wide(&mut [], &w("a")));
    }

    #[test]
    fn win32_errors_map_to_removed_or_failed() {
        assert_eq!(read_error_code(ERROR_DEVICE_NOT_CONNECTED), XKHID_E_DEVICE_REMOVED);
        assert_eq!(read_error_code(ERROR_OPERATION_ABORTED), XKHID_E_DEVICE_REMOVED);
        assert_eq!(read_error_code(87), XKHID_E_READ_FAILED);
        assert_eq!(write_error_code(ERROR_INVALID_HANDLE), XKHID_E_DEVICE_REMOVED);
        assert_eq!(write_error_code(87), XKHID_E_WRITE_FAILED);
    }
}
