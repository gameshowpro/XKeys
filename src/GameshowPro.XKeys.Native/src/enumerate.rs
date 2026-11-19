// Copyright (c) Barjonas LLC 2026.
//! Enumeration of HID interfaces into a caller-supplied array.
//!
//! Every allocation and handle made here is released before [`enumerate`] returns, on every path (RAII guards): the SetupAPI
//! device list, each interface handle, and each block of preparsed data. Nothing is allocated on the Rust heap. This is the
//! reason the crate exists: the vendor library leaked an `AllocHGlobal` per HID interface and the preparsed data per P.I.
//! interface on every call.

use crate::report::{
    DeviceInfo, PATH_CHARS, STRING_CHARS, XKHID_E_BUFFER_TOO_SMALL, XKHID_E_ENUMERATION_FAILED, XKHID_OK, caps_match, copy_wide,
    path_has_vid,
};
use crate::win32::*;
use core::ffi::c_void;
use core::ptr;

/// Destroys a SetupAPI device information set when dropped.
struct DeviceInfoSet(isize);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: a valid set from SetupDiGetClassDevsW, destroyed once.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// `SP_DEVICE_INTERFACE_DETAIL_DATA_W` with room for a path of [`PATH_CHARS`] units (terminator included), on the stack.
#[repr(C)]
struct DetailBuffer {
    cb_size: u32,
    path: [u16; PATH_CHARS],
}

/// Find every present HID interface of vendor `vid` whose top-level collection passes the filter (`usage_page` and `output_len`,
/// 0 meaning any), and store up to `max` of them in `out`. Returns the `XKHID_*` result and the number that matched, which may
/// exceed `max` (then the result is `XKHID_E_BUFFER_TOO_SMALL` and the first `max` are stored).
///
/// Other vendors' interfaces are skipped by their path (see [`path_has_vid`]) and never opened; matching ones are opened with
/// no access rights (so a keyboard or mouse collection held exclusively by Windows can still be described) and closed again.
///
/// # Safety
/// `out` must be valid for writing `max` entries (it may be null when `max` is 0).
pub unsafe fn enumerate(vid: u16, usage_page: u16, output_len: u16, out: *mut DeviceInfo, max: u32) -> (i32, u32) {
    // SAFETY: plain SetupAPI call; the set is destroyed by the guard.
    let raw = unsafe { SetupDiGetClassDevsW(&GUID_DEVINTERFACE_HID, ptr::null(), ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
    if raw == -1 {
        return (XKHID_E_ENUMERATION_FAILED, 0);
    }
    let set = DeviceInfoSet(raw);
    let mut matched: u32 = 0;
    let mut detail = DetailBuffer { cb_size: 0, path: [0; PATH_CHARS] };
    let mut info = DeviceInfo::ZERO;

    for index in 0u32.. {
        let mut data = SpDeviceInterfaceData {
            cb_size: size_of::<SpDeviceInterfaceData>() as u32,
            interface_class_guid: Guid { data1: 0, data2: 0, data3: 0, data4: [0; 8] },
            flags: 0,
            reserved: 0,
        };
        // SAFETY: `set` is live; `data` is a valid, sized out structure.
        if unsafe { SetupDiEnumDeviceInterfaces(set.0, ptr::null(), &GUID_DEVINTERFACE_HID, index, &mut data) } == 0 {
            // ERROR_NO_MORE_ITEMS ends the list; any other failure also ends it, with what was found so far.
            break;
        }
        detail.cb_size = SP_DEVICE_INTERFACE_DETAIL_DATA_W_SIZE;
        detail.path[0] = 0;
        // SAFETY: `detail` is a writable buffer of the stated size whose first field is cbSize. A path that does not fit fails
        // with ERROR_INSUFFICIENT_BUFFER and is skipped: it could not be stored in DeviceInfo::path anyway.
        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                set.0,
                &data,
                (&raw mut detail).cast::<c_void>(),
                size_of::<DetailBuffer>() as u32,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            continue;
        }
        let path_len = detail.path.iter().position(|&c| c == 0).unwrap_or(PATH_CHARS);
        if path_len == PATH_CHARS || !path_has_vid(&detail.path[..path_len], vid) {
            continue;
        }

        // SAFETY: `detail.path` is NUL-terminated (checked above). No access rights: only attributes, caps and strings are read.
        let device = OwnedHandle(unsafe {
            CreateFileW(detail.path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, 0, ptr::null_mut())
        });
        if !device.is_valid() {
            continue;
        }
        // SAFETY: `device` is a live HID interface handle.
        let Some((attributes, caps)) = (unsafe { query_hid(device.0) }) else { continue };
        if attributes.vendor_id != vid || !caps_match(usage_page, output_len, caps.usage_page, caps.output_report_byte_length) {
            continue;
        }

        matched += 1;
        if matched > max {
            continue;
        }
        info.path.fill(0);
        copy_wide(&mut info.path, &detail.path[..path_len]);
        info.vid = attributes.vendor_id;
        info.pid = attributes.product_id;
        info.version = attributes.version_number;
        info.usage_page = caps.usage_page;
        info.usage = caps.usage;
        info.input_len = caps.input_report_byte_length;
        info.output_len = caps.output_report_byte_length;
        info.feature_len = caps.feature_report_byte_length;
        // SAFETY: `device` is live; each buffer is STRING_CHARS units, passed as its byte size.
        unsafe {
            read_string(device.0, HidD_GetManufacturerString, &mut info.manufacturer);
            read_string(device.0, HidD_GetProductString, &mut info.product);
            ptr::write(out.add(matched as usize - 1), info);
        }
    }
    drop(set);
    (if matched > max { XKHID_E_BUFFER_TOO_SMALL } else { XKHID_OK }, matched)
}

type StringGetter = unsafe extern "system" fn(Handle, *mut c_void, u32) -> u8;

/// Read one of the device's strings into `dst`, NUL-terminated; empty if the device does not answer.
///
/// # Safety
/// `device` must be a live HID interface handle.
unsafe fn read_string(device: Handle, get: StringGetter, dst: &mut [u16; STRING_CHARS]) {
    dst.fill(0);
    // SAFETY: `dst` is writable for its full byte size.
    let ok = unsafe { get(device, dst.as_mut_ptr().cast::<c_void>(), (STRING_CHARS * 2) as u32) };
    if ok == 0 {
        dst[0] = 0;
    }
    // A string that fills the buffer is not terminated by Windows; cut it.
    dst[STRING_CHARS - 1] = 0;
}
