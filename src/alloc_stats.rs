//! Built only with `--features alloc-stats`: counts the bytes the program
//! holds on its heap, Lua included (mlua allocates through Rust). A soak
//! compares this with the private bytes Windows reports: live heap bytes
//! that climb mean a leak; flat heap bytes under rising private bytes point
//! at the allocator or at memory outside the Rust heap.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub struct Counting;

static LIVE: AtomicI64 = AtomicI64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counters are only updated around it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as i64, Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        p
    }
}

/// Bytes allocated and not yet freed, and the number of allocations so far.
pub fn snapshot() -> (i64, u64) {
    (LIVE.load(Ordering::Relaxed), CALLS.load(Ordering::Relaxed))
}

/// The committed bytes of all Windows heaps of the process, and the bytes
/// allocated in them. Committed minus allocated is free space inside the
/// heaps: when it grows while the allocated bytes stay flat, the heap is
/// fragmenting.
#[cfg(windows)]
pub fn heaps() -> (usize, u64, u64) {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Memory::{GetProcessHeaps, HEAP_SUMMARY, HeapSummary};
    let mut handles: Vec<HANDLE> = vec![std::ptr::null_mut(); 64];
    // SAFETY: the buffer holds `handles.len()` HANDLE slots.
    let n = unsafe { GetProcessHeaps(handles.len() as u32, handles.as_mut_ptr()) } as usize;
    let (mut allocated, mut committed) = (0u64, 0u64);
    for h in handles.iter().take(n.min(handles.len())) {
        let mut s = HEAP_SUMMARY {
            cb: size_of::<HEAP_SUMMARY>() as u32,
            cbAllocated: 0,
            cbCommitted: 0,
            cbReserved: 0,
            cbMaxReserve: 0,
        };
        // SAFETY: `h` came from GetProcessHeaps and `s` has its size set.
        if unsafe { HeapSummary(*h, 0, &mut s) } != 0 {
            allocated += s.cbAllocated as u64;
            committed += s.cbCommitted as u64;
        }
    }
    (n, allocated, committed)
}

#[cfg(not(windows))]
pub fn heaps() -> (usize, u64, u64) {
    (0, 0, 0)
}
