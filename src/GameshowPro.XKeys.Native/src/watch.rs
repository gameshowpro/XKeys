// Copyright (c) Barjonas LLC 2026.
//! HID interface arrival and removal, without a window: `CM_Register_Notification` on the HID interface class.
//!
//! Windows calls [`on_notification`] on a thread-pool thread for every HID interface that arrives or leaves. The callback
//! keeps only those whose path names the watched vendor, bumps a generation counter and signals an auto-reset event; the
//! consumer waits on the event and re-enumerates when the generation has moved. It never calls into managed code.

use crate::report::path_has_vid;
use crate::win32::*;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

/// The part of a watch the callback reads. Boxed so its address is stable for the registration's lifetime.
struct WatchShared {
    /// Auto-reset; signaled after every counted arrival or removal.
    event: OwnedHandle,
    generation: AtomicU64,
    vid: u16,
}

/// What `xkhid_watch_create` returns and the other watch exports take. Opaque to callers.
pub struct Watch {
    notify: isize,
    /// From `Box::into_raw`; freed by `Drop` after the registration is gone. A raw pointer rather than a `Box`, because the
    /// callback reads it on another thread while the watch exists.
    shared: *mut WatchShared,
}

// SAFETY: the shared state is only read (atomics and a kernel handle) by the callback and the owner.
unsafe impl Send for Watch {}
unsafe impl Sync for Watch {}

/// The longest symbolic link the callback inspects, in UTF-16 units; HID paths are far shorter.
const MAX_LINK_CHARS: usize = 1024;

unsafe extern "system" fn on_notification(
    _notify: isize,
    context: *mut c_void,
    action: u32,
    data: *const CmNotifyEventDataHead,
    size: u32,
) -> u32 {
    if context.is_null() || data.is_null() {
        return 0;
    }
    if action != CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL && action != CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL {
        return 0;
    }
    // SAFETY: `context` is the WatchShared registered with this callback; `Watch::destroy` unregisters (which waits for running
    // callbacks) before freeing it.
    let shared = unsafe { &*context.cast_const().cast::<WatchShared>() };
    // The symbolic link follows the fixed head, NUL-terminated, within `size` bytes.
    let head = size_of::<CmNotifyEventDataHead>();
    let available = (size as usize).saturating_sub(head) / 2;
    // SAFETY: Windows passes `size` readable bytes at `data`; the link lies after the head. Only whole UTF-16 units inside that
    // range are read, and at most MAX_LINK_CHARS of them.
    let link = unsafe { core::slice::from_raw_parts(data.cast::<u8>().add(head).cast::<u16>(), available.min(MAX_LINK_CHARS)) };
    let link = &link[..link.iter().position(|&c| c == 0).unwrap_or(link.len())];
    if path_has_vid(link, shared.vid) {
        shared.generation.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the event lives as long as `shared`.
        unsafe { SetEvent(shared.event.0) };
    }
    0
}

impl Watch {
    /// Start watching vendor `vid`'s HID interfaces. `None` if the event or the registration could not be made.
    pub fn create(vid: u16) -> Option<Box<Watch>> {
        // SAFETY: plain Win32 call: auto-reset, not signaled.
        let event = OwnedHandle(unsafe { CreateEventW(core::ptr::null(), 0, 0, core::ptr::null()) });
        if !event.is_valid() {
            return None;
        }
        let shared = Box::into_raw(Box::new(WatchShared { event, generation: AtomicU64::new(0), vid }));
        let filter = CmNotifyFilter {
            cb_size: size_of::<CmNotifyFilter>() as u32,
            flags: 0,
            filter_type: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            reserved: 0,
            class_guid: GUID_DEVINTERFACE_HID,
            union_rest: [0; 400 - 16],
        };
        let mut notify = 0isize;
        // SAFETY: `filter` is a valid, sized filter; the context pointer stays valid until CM_Unregister_Notification returns.
        let result = unsafe { CM_Register_Notification(&filter, shared.cast::<c_void>(), on_notification, &mut notify) };
        if result != CR_SUCCESS || notify == 0 {
            // SAFETY: not registered, so nothing else holds the pointer.
            drop(unsafe { Box::from_raw(shared) });
            return None;
        }
        Some(Box::new(Watch { notify, shared }))
    }

    /// Arrivals plus removals counted since the watch was made.
    pub fn generation(&self) -> u64 {
        // SAFETY: `shared` lives as long as `self`.
        unsafe { (*self.shared).generation.load(Ordering::SeqCst) }
    }

    /// A duplicate of the event for the caller to own, or null.
    pub fn event_handle(&self) -> Handle {
        // SAFETY: `shared` and its event are live while `self` is.
        unsafe { duplicate((*self.shared).event.0) }
    }
}

impl Drop for Watch {
    /// Unregister, which waits for a callback still running, then free. Must not run inside the callback.
    fn drop(&mut self) {
        // SAFETY: `notify` is this watch's registration, unregistered once; after it returns no callback is running or will
        // start, so the shared state (and its event) is ours alone to free.
        unsafe {
            CM_Unregister_Notification(self.notify);
            drop(Box::from_raw(self.shared));
        }
    }
}
