// Copyright (c) Barjonas LLC 2026.
//! An open device: its read thread, ring and event, and the synchronous write.
//!
//! The read thread keeps one overlapped `ReadFile` outstanding. When it completes, the thread reads `QueryPerformanceCounter`
//! before doing anything else, then copies the report into the ring, signals the event, and issues the next read. Shutdown sets
//! a stop event the thread waits on alongside the read; the thread cancels its read with `CancelIoEx` and waits for the
//! cancellation to complete before it returns, so the buffer and `OVERLAPPED` (on its stack) are never freed under a pending
//! read. [`Device::close`] joins the thread before any handle is closed.

use crate::report::*;
use crate::ring::Ring;
use crate::win32::*;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// State shared by the device's owner (the consumer) and its read thread. Dropped, closing its handles, after both let go,
/// which is after the read thread has returned.
struct Shared {
    ring: Ring<ReportEntry>,
    /// Auto-reset; signaled after every entry the read thread produces (or drops).
    ring_event: OwnedHandle,
    /// Manual-reset; set by [`Device::close`].
    stop_event: OwnedHandle,
    /// Manual-reset; the read's `OVERLAPPED` event.
    read_event: OwnedHandle,
    /// Opened GENERIC_READ, overlapped.
    read_handle: OwnedHandle,
    stopping: AtomicBool,
    input_len: usize,
    timestamp_offset: i32,
    flags: u32,
    pid: u16,
}

// SAFETY: the handles are kernel handles, usable from any thread; the ring is used under its single-producer (read thread),
// single-consumer (the caller's `read`) contract.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// What `xkhid_open` returns and the other device exports take. Opaque to callers.
pub struct Device {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Opened GENERIC_WRITE, overlapped; invalid when the interface has no output report.
    write_handle: OwnedHandle,
    input_len: u16,
    output_len: u16,
}

fn create_event(manual_reset: bool) -> OwnedHandle {
    // SAFETY: plain Win32 call; null attributes and name are allowed. Not signaled.
    OwnedHandle(unsafe { CreateEventW(ptr::null(), i32::from(manual_reset), 0, ptr::null()) })
}

fn open_path(path: *const u16, access: u32) -> OwnedHandle {
    // SAFETY: the caller of `Device::open` promised `path` is NUL-terminated.
    OwnedHandle(unsafe {
        CreateFileW(path, access, FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, ptr::null_mut())
    })
}

impl Device {
    /// Open the interface at `path`. See `xkhid_open`.
    ///
    /// # Safety
    /// `path` must be a NUL-terminated UTF-16 string.
    pub unsafe fn open(path: *const u16, ring_capacity: u32, flags: u32, timestamp_offset: i32) -> Result<Box<Device>, i32> {
        if flags & !OPEN_ALL_FLAGS != 0 {
            return Err(XKHID_E_BAD_ARGUMENT);
        }
        let read_handle = open_path(path, GENERIC_READ);
        if !read_handle.is_valid() {
            // SAFETY: plain Win32 call, right after the failure.
            let error = unsafe { GetLastError() };
            return Err(match error {
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NO_SUCH_DEVICE => XKHID_E_NOT_FOUND,
                _ => XKHID_E_OPEN_READ_FAILED,
            });
        }
        // SAFETY: `read_handle` is a live handle; a non-HID path fails here.
        let Some((attributes, caps)) = (unsafe { query_hid(read_handle.0) }) else { return Err(XKHID_E_OPEN_READ_FAILED) };
        let input_len = caps.input_report_byte_length;
        let output_len = caps.output_report_byte_length;
        if input_len == 0 {
            return Err(XKHID_E_NO_INPUT_REPORT);
        }
        if usize::from(input_len) > MAX_REPORT_LEN {
            return Err(XKHID_E_REPORT_TOO_LONG);
        }
        if timestamp_offset >= 0 && timestamp_offset as usize + 4 > usize::from(input_len) {
            return Err(XKHID_E_BAD_ARGUMENT);
        }
        let write_handle = if output_len > 0 {
            let handle = open_path(path, GENERIC_WRITE);
            if !handle.is_valid() {
                return Err(XKHID_E_OPEN_WRITE_FAILED);
            }
            handle
        } else {
            OwnedHandle(ptr::null_mut())
        };
        let (ring_event, stop_event, read_event) = (create_event(false), create_event(true), create_event(true));
        if !ring_event.is_valid() || !stop_event.is_valid() || !read_event.is_valid() {
            return Err(XKHID_E_OUT_OF_MEMORY);
        }
        let shared = Arc::new(Shared {
            ring: Ring::new(ring_capacity as usize, ReportEntry::ZERO),
            ring_event,
            stop_event,
            read_event,
            read_handle,
            stopping: AtomicBool::new(false),
            input_len: usize::from(input_len),
            timestamp_offset: timestamp_offset.max(-1),
            flags,
            pid: attributes.product_id,
        });
        let for_thread = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(format!("xkhid read {:04X}", attributes.product_id))
            .stack_size(64 * 1024)
            .spawn(move || read_thread(&for_thread))
            .map_err(|_| XKHID_E_OUT_OF_MEMORY)?;
        Ok(Box::new(Device { shared, thread: Some(thread), write_handle, input_len, output_len }))
    }

    /// Stop the read thread and wait for it, then free everything (see `Drop`).
    pub fn close(self: Box<Device>) {
        drop(self);
    }

    pub fn input_len(&self) -> u16 {
        self.input_len
    }

    pub fn output_len(&self) -> u16 {
        self.output_len
    }

    pub fn pid(&self) -> u16 {
        self.shared.pid
    }

    pub fn dropped(&self) -> u64 {
        self.shared.ring.dropped()
    }

    /// A duplicate of the ring event for the caller to own, or null.
    pub fn event_handle(&self) -> Handle {
        // SAFETY: the event is live while `self` is.
        unsafe { duplicate(self.shared.ring_event.0) }
    }

    /// Move up to `max` entries, oldest first, into `out`.
    ///
    /// # Safety
    /// `out` must be valid for writing `max` entries, and calls for one device must not overlap.
    pub unsafe fn read(&self, out: *mut ReportEntry, max: usize) -> usize {
        // SAFETY: forwarded from the caller; the caller's thread is the ring's only consumer.
        unsafe { self.shared.ring.read_into(out, max) }
    }

    /// Write one output report and wait up to `timeout_ms` (`u32::MAX`: forever) for it to complete. Returns the `XKHID_*` result
    /// and the Win32 error behind a failure (0 otherwise). Calls may overlap each other but not `close`.
    pub fn write(&self, data: &[u8], timeout_ms: u32) -> (i32, u32) {
        if !self.write_handle.is_valid() || data.len() != usize::from(self.output_len) {
            return (XKHID_E_WRITE_LENGTH, 0);
        }
        let event = create_event(true);
        if !event.is_valid() {
            return (XKHID_E_OUT_OF_MEMORY, 0);
        }
        let handle = self.write_handle.0;
        let mut overlapped = Overlapped::with_event(event.0);
        let mut written = 0u32;
        // SAFETY: `data` and `overlapped` stay alive until the write has completed or been cancelled and reaped below: every
        // path that returns after a pending write first waits for it with GetOverlappedResult(wait = TRUE).
        unsafe {
            if WriteFile(handle, data.as_ptr().cast::<c_void>(), data.len() as u32, ptr::null_mut(), &mut overlapped) == 0 {
                let error = GetLastError();
                if error != ERROR_IO_PENDING {
                    return (write_error_code(error), error);
                }
                if WaitForSingleObject(event.0, timeout_ms) != WAIT_OBJECT_0 {
                    CancelIoEx(handle, &mut overlapped);
                    // Reap the write: either the cancellation or a completion that beat it.
                    if GetOverlappedResult(handle, &mut overlapped, &mut written, 1) == 0 {
                        let error = GetLastError();
                        return if error == ERROR_OPERATION_ABORTED { (XKHID_E_WRITE_TIMEOUT, 0) } else { (write_error_code(error), error) };
                    }
                    return finish_write(written, data.len());
                }
            }
            if GetOverlappedResult(handle, &mut overlapped, &mut written, 1) == 0 {
                let error = GetLastError();
                return (write_error_code(error), error);
            }
        }
        finish_write(written, data.len())
    }
}

impl Drop for Device {
    /// Stop the read thread and wait for it; then the fields' drops close the write handle and (last reference) `shared`
    /// closes the read handle and the events.
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        // SAFETY: the stop event is live until `shared` is dropped, after the join.
        unsafe { SetEvent(self.shared.stop_event.0) };
        if let Some(thread) = self.thread.take() {
            // The thread cannot panic (panic = abort ends the process), so the result carries nothing.
            let _ = thread.join();
        }
    }
}

fn finish_write(written: u32, expected: usize) -> (i32, u32) {
    if written as usize == expected { (XKHID_OK, 0) } else { (XKHID_E_WRITE_FAILED, 0) }
}

/// Raises the calling thread for the read loop, and undoes MMCSS registration when dropped.
struct ThreadBoost(Handle);

impl ThreadBoost {
    fn apply(mmcss: bool) -> ThreadBoost {
        // SAFETY: plain Win32 calls on the current thread. Failure leaves the thread at normal priority, which only costs timing.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            if mmcss {
                let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
                let mut index = 0u32;
                return ThreadBoost(AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index));
            }
        }
        ThreadBoost(ptr::null_mut())
    }
}

impl Drop for ThreadBoost {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this thread.
            unsafe { AvRevertMmThreadCharacteristics(self.0) };
        }
    }
}

/// The device's read loop. Returns when [`Device::close`] stops it or a read fails (after queueing one error entry).
fn read_thread(shared: &Shared) {
    let _boost = ThreadBoost::apply(shared.flags & OPEN_MMCSS != 0);
    let suppress = shared.flags & OPEN_SUPPRESS_DUPLICATES != 0;
    let handle = shared.read_handle.0;
    let waits = [shared.read_event.0, shared.stop_event.0];
    let mut buffer = [0u8; MAX_REPORT_LEN];
    let mut previous = [0u8; MAX_REPORT_LEN];
    let mut previous_len = usize::MAX;
    let mut sequence: u32 = 0;

    // Ends the session: one error entry, which always fits (it uses the slot reports leave free), then the event.
    let fail = |sequence: u32, win32_error: u32| {
        let stamp = qpc();
        if !shared.stopping.load(Ordering::SeqCst) {
            let entry = ReportEntry::error(stamp, sequence, read_error_code(win32_error), win32_error);
            // SAFETY: this thread is the ring's only producer.
            unsafe {
                shared.ring.push(&entry, 0);
                SetEvent(shared.ring_event.0);
            }
        }
    };

    while !shared.stopping.load(Ordering::SeqCst) {
        let mut overlapped = Overlapped::with_event(shared.read_event.0);
        let stamp;
        let queued;
        // SAFETY: `buffer` and `overlapped` live on this stack frame until the read is complete or cancelled and reaped: every
        // path out of this block after a pending read waits for it with GetOverlappedResult(wait = TRUE).
        unsafe {
            if ReadFile(handle, buffer.as_mut_ptr().cast::<c_void>(), shared.input_len as u32, ptr::null_mut(), &mut overlapped) != 0 {
                // Completed at once: the report was already queued in the HID class driver.
                stamp = qpc();
                queued = true;
            } else {
                let error = GetLastError();
                if error != ERROR_IO_PENDING {
                    fail(sequence, error);
                    return;
                }
                let woke = WaitForMultipleObjects(2, waits.as_ptr(), 0, INFINITE);
                // First thing after the wait: nothing else may delay the stamp.
                stamp = qpc();
                queued = false;
                if woke != WAIT_OBJECT_0 {
                    // Stop requested (or the wait itself failed): cancel the read and wait until it is no longer pending.
                    let wait_error = if woke == WAIT_FAILED { GetLastError() } else { 0 };
                    let mut ignored = 0u32;
                    CancelIoEx(handle, &mut overlapped);
                    GetOverlappedResult(handle, &mut overlapped, &mut ignored, 1);
                    // `fail` does nothing when the stop was requested.
                    fail(sequence, wait_error);
                    return;
                }
            }
            let mut read = 0u32;
            if GetOverlappedResult(handle, &mut overlapped, &mut read, 0) == 0 {
                fail(sequence, GetLastError());
                return;
            }
            let report = &buffer[..(read as usize).min(MAX_REPORT_LEN)];
            if suppress && report.len() == previous_len && same_ignoring_timestamp(report, &previous[..previous_len], shared.timestamp_offset)
            {
                continue;
            }
            if suppress {
                previous[..report.len()].copy_from_slice(report);
                previous_len = report.len();
            }
            let entry = ReportEntry::report(stamp, sequence, queued, report, shared.timestamp_offset);
            sequence = sequence.wrapping_add(1);
            // A full ring drops the report and counts it (the sequence gap shows where); the read thread never blocks on the
            // consumer. The event is signaled either way so the consumer drains what is there.
            shared.ring.push(&entry, 1);
            SetEvent(shared.ring_event.0);
        }
    }
}
