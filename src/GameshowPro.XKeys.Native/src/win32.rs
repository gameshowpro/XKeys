// Copyright (c) Barjonas LLC 2026.
//! The Win32 functions and structures the crate uses, declared by hand (no dependencies). `hid.dll`, `setupapi.dll`,
//! `cfgmgr32.dll` and `avrt.dll` are linked with `raw-dylib`, so no Windows SDK import library is needed for them.
#![allow(non_snake_case, clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Bool = i32;

pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const FILE_SHARE_READ: u32 = 0x0000_0001;
pub const FILE_SHARE_WRITE: u32 = 0x0000_0002;
pub const OPEN_EXISTING: u32 = 3;
pub const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_IO_PENDING: u32 = 997;
pub const ERROR_NO_MORE_ITEMS: u32 = 259;

pub const INFINITE: u32 = 0xFFFF_FFFF;
pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_TIMEOUT: u32 = 0x102;
pub const WAIT_FAILED: u32 = 0xFFFF_FFFF;

pub const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
pub const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;

pub const DIGCF_PRESENT: u32 = 0x0000_0002;
pub const DIGCF_DEVICEINTERFACE: u32 = 0x0000_0010;

pub const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;

pub const CR_SUCCESS: u32 = 0;
pub const CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE: u32 = 0;
pub const CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL: u32 = 0;
pub const CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// `GUID_DEVINTERFACE_HID` {4D1E55B2-F16F-11CF-88CB-001111000030}, the value `HidD_GetHidGuid` returns.
pub const GUID_DEVINTERFACE_HID: Guid =
    Guid { data1: 0x4D1E_55B2, data2: 0xF16F, data3: 0x11CF, data4: [0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30] };

#[repr(C)]
pub struct Overlapped {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u32,
    pub offset_high: u32,
    pub event: Handle,
}

impl Overlapped {
    pub fn with_event(event: Handle) -> Overlapped {
        Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event }
    }
}

#[repr(C)]
pub struct SpDeviceInterfaceData {
    pub cb_size: u32,
    pub interface_class_guid: Guid,
    pub flags: u32,
    pub reserved: usize,
}

/// `cbSize` of `SP_DEVICE_INTERFACE_DETAIL_DATA_W`: the size of its fixed part (DWORD + one WCHAR, padded), 8 on 64-bit and 6
/// on 32-bit Windows.
pub const SP_DEVICE_INTERFACE_DETAIL_DATA_W_SIZE: u32 = if cfg!(target_pointer_width = "64") { 8 } else { 6 };

#[repr(C)]
pub struct HiddAttributes {
    pub size: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
}

#[repr(C)]
#[derive(Default)]
pub struct HidpCaps {
    pub usage: u16,
    pub usage_page: u16,
    pub input_report_byte_length: u16,
    pub output_report_byte_length: u16,
    pub feature_report_byte_length: u16,
    pub reserved: [u16; 17],
    pub number_link_collection_nodes: u16,
    pub number_input_button_caps: u16,
    pub number_input_value_caps: u16,
    pub number_input_data_indices: u16,
    pub number_output_button_caps: u16,
    pub number_output_value_caps: u16,
    pub number_output_data_indices: u16,
    pub number_feature_button_caps: u16,
    pub number_feature_value_caps: u16,
    pub number_feature_data_indices: u16,
}

/// `CM_NOTIFY_FILTER` with the `DeviceInterface` member of its union (the union is 400 bytes: `DeviceInstance.InstanceId`).
#[repr(C)]
pub struct CmNotifyFilter {
    pub cb_size: u32,
    pub flags: u32,
    pub filter_type: u32,
    pub reserved: u32,
    pub class_guid: Guid,
    pub union_rest: [u8; 400 - 16],
}

/// The fixed head of `CM_NOTIFY_EVENT_DATA` for a device-interface event; the NUL-terminated symbolic link follows it.
#[repr(C)]
pub struct CmNotifyEventDataHead {
    pub filter_type: u32,
    pub reserved: u32,
    pub class_guid: Guid,
}

pub type CmNotifyCallback =
    unsafe extern "system" fn(notify: isize, context: *mut c_void, action: u32, data: *const CmNotifyEventDataHead, size: u32) -> u32;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn QueryPerformanceCounter(count: *mut i64) -> Bool;
    pub fn QueryPerformanceFrequency(frequency: *mut i64) -> Bool;
    pub fn CreateEventW(attributes: *const c_void, manual_reset: Bool, initial_state: Bool, name: *const u16) -> Handle;
    pub fn SetEvent(event: Handle) -> Bool;
    pub fn CloseHandle(handle: Handle) -> Bool;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetCurrentThread() -> Handle;
    pub fn SetThreadPriority(thread: Handle, priority: i32) -> Bool;
    pub fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        desired_access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    pub fn GetLastError() -> u32;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub fn ReadFile(file: Handle, buffer: *mut c_void, length: u32, read: *mut u32, overlapped: *mut Overlapped) -> Bool;
    pub fn WriteFile(file: Handle, buffer: *const c_void, length: u32, written: *mut u32, overlapped: *mut Overlapped) -> Bool;
    pub fn GetOverlappedResult(file: Handle, overlapped: *mut Overlapped, transferred: *mut u32, wait: Bool) -> Bool;
    pub fn CancelIoEx(file: Handle, overlapped: *mut Overlapped) -> Bool;
    pub fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    pub fn WaitForMultipleObjects(count: u32, handles: *const Handle, wait_all: Bool, milliseconds: u32) -> u32;
}

#[link(name = "hid", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn HidD_GetHidGuid(guid: *mut Guid);
    pub fn HidD_GetAttributes(device: Handle, attributes: *mut HiddAttributes) -> u8;
    pub fn HidD_GetPreparsedData(device: Handle, preparsed: *mut isize) -> u8;
    pub fn HidD_FreePreparsedData(preparsed: isize) -> u8;
    pub fn HidP_GetCaps(preparsed: isize, caps: *mut HidpCaps) -> i32;
    pub fn HidD_GetManufacturerString(device: Handle, buffer: *mut c_void, length: u32) -> u8;
    pub fn HidD_GetProductString(device: Handle, buffer: *mut c_void, length: u32) -> u8;
}

#[link(name = "setupapi", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn SetupDiGetClassDevsW(class: *const Guid, enumerator: *const u16, parent: Handle, flags: u32) -> isize;
    pub fn SetupDiEnumDeviceInterfaces(
        set: isize,
        device: *const c_void,
        class: *const Guid,
        index: u32,
        data: *mut SpDeviceInterfaceData,
    ) -> Bool;
    pub fn SetupDiGetDeviceInterfaceDetailW(
        set: isize,
        data: *const SpDeviceInterfaceData,
        detail: *mut c_void,
        detail_size: u32,
        required: *mut u32,
        device: *mut c_void,
    ) -> Bool;
    pub fn SetupDiDestroyDeviceInfoList(set: isize) -> Bool;
}

#[link(name = "cfgmgr32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn CM_Register_Notification(
        filter: *const CmNotifyFilter,
        context: *const c_void,
        callback: CmNotifyCallback,
        notify: *mut isize,
    ) -> u32;
    pub fn CM_Unregister_Notification(notify: isize) -> u32;
}

#[link(name = "avrt", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn AvSetMmThreadCharacteristicsW(task: *const u16, task_index: *mut u32) -> Handle;
    pub fn AvRevertMmThreadCharacteristics(handle: Handle) -> Bool;
}

/// `QueryPerformanceCounter`, which cannot fail on Windows XP or later.
#[inline(always)]
pub fn qpc() -> i64 {
    let mut ticks = 0;
    // SAFETY: `ticks` is a valid out pointer.
    unsafe { QueryPerformanceCounter(&mut ticks) };
    ticks
}

/// Closes a kernel handle when dropped. Null and `INVALID_HANDLE_VALUE` are ignored.
pub struct OwnedHandle(pub Handle);

impl OwnedHandle {
    pub fn is_valid(&self) -> bool {
        !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// A duplicate of `handle` for the caller to own and close, or null.
///
/// # Safety
/// `handle` must be a live handle.
pub unsafe fn duplicate(handle: Handle) -> Handle {
    let mut copy: Handle = core::ptr::null_mut();
    // SAFETY: the pseudo-handle from GetCurrentProcess needs no closing; `copy` is a valid out pointer.
    let ok = unsafe {
        let process = GetCurrentProcess();
        DuplicateHandle(process, handle, process, &mut copy, 0, 0, DUPLICATE_SAME_ACCESS)
    };
    if ok == 0 { core::ptr::null_mut() } else { copy }
}

/// Frees HID preparsed data when dropped.
pub struct Preparsed(pub isize);

impl Drop for Preparsed {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: obtained from HidD_GetPreparsedData and freed once.
            unsafe { HidD_FreePreparsedData(self.0) };
        }
    }
}

/// The interface's attributes and top-level-collection capabilities, read through `device` (any access, even none).
///
/// # Safety
/// `device` must be a live handle to a HID interface.
pub unsafe fn query_hid(device: Handle) -> Option<(HiddAttributes, HidpCaps)> {
    let mut attributes = HiddAttributes { size: size_of::<HiddAttributes>() as u32, vendor_id: 0, product_id: 0, version_number: 0 };
    // SAFETY: valid out pointers; the preparsed data is freed by the guard on every path.
    unsafe {
        if HidD_GetAttributes(device, &mut attributes) == 0 {
            return None;
        }
        let mut raw = 0isize;
        if HidD_GetPreparsedData(device, &mut raw) == 0 {
            return None;
        }
        let preparsed = Preparsed(raw);
        let mut caps = HidpCaps::default();
        if HidP_GetCaps(preparsed.0, &mut caps) != HIDP_STATUS_SUCCESS {
            return None;
        }
        Some((attributes, caps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn structure_sizes_match_the_windows_headers() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(size_of::<Overlapped>(), 32);
        assert_eq!(size_of::<SpDeviceInterfaceData>(), 32);
        assert_eq!(size_of::<HiddAttributes>(), 12);
        assert_eq!(size_of::<HidpCaps>(), 64);
        assert_eq!(offset_of!(HidpCaps, number_link_collection_nodes), 44);
        assert_eq!(size_of::<CmNotifyFilter>(), 416);
        assert_eq!(offset_of!(CmNotifyFilter, class_guid), 16);
        assert_eq!(size_of::<CmNotifyEventDataHead>(), 24);
    }

    #[test]
    fn the_hid_class_guid_is_the_one_windows_reports() {
        let mut guid = Guid { data1: 0, data2: 0, data3: 0, data4: [0; 8] };
        // SAFETY: valid out pointer.
        unsafe { HidD_GetHidGuid(&mut guid) };
        assert_eq!(guid, GUID_DEVINTERFACE_HID);
    }
}
