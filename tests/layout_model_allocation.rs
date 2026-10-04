//! A damaged or hostile layout model must be rejected before the parser reserves the tables its
//! header announces.
//!
//! This binary installs a counting allocator. That needs `unsafe`: `GlobalAlloc` is an unsafe trait
//! and there is no safe way to observe allocation sizes. The wrapper forwards every call unchanged
//! to `System` and only records sizes. It is test-only code and lives in its own test binary so that
//! no other test shares the allocator or its counter.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use autokeyboardlayot::layout_model::{LayoutModel, LayoutModelError};

struct CountingAllocator;

static LARGEST_ALLOCATION: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method passes its arguments unchanged to `System`, so the `GlobalAlloc` contract
// holds exactly when `System` upholds it; the wrapper only records the requested size in an atomic.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LARGEST_ALLOCATION.fetch_max(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`; the layout is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        LARGEST_ALLOCATION.fetch_max(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`; the layout is forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LARGEST_ALLOCATION.fetch_max(new_size, Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::realloc`; all arguments are forwarded unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`; the pointer and layout are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Anything larger than this during a failed parse means the announced tables were reserved.
const FAILED_PARSE_ALLOCATION_LIMIT: usize = 64 * 1024;

const REAL_MODEL: &[u8] = include_bytes!("../data/layout-model/en-ru-et.aklm");

/// A well-formed header that announces the largest tables the parser accepts, followed by no data.
fn hostile_header() -> Vec<u8> {
    let mut bytes = b"AKLM".to_vec();
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.push(3);
    for id in ["en-US", "ru-RU", "et-EE"] {
        bytes.push(id.len() as u8);
        bytes.extend_from_slice(id.as_bytes());
    }
    bytes.extend_from_slice(&(1u32 << 20).to_le_bytes()); // buckets
    bytes.extend_from_slice(&64u16.to_le_bytes()); // dim
    bytes.extend_from_slice(&256u16.to_le_bytes()); // hidden
    bytes.extend_from_slice(&[4, 16, 4]); // max_n, max_ngrams, context
    bytes
}

/// Parses `bytes`, expects `Format`, and returns the largest single allocation made meanwhile.
fn rejected_with_largest_allocation(bytes: &[u8]) -> usize {
    LARGEST_ALLOCATION.store(0, Ordering::Relaxed);
    let result = LayoutModel::parse(bytes);
    let largest = LARGEST_ALLOCATION.load(Ordering::Relaxed);
    assert_eq!(
        result.unwrap_err(),
        LayoutModelError::Format,
        "{} bytes must be rejected as malformed",
        bytes.len()
    );
    largest
}

// One test only: the counter is process-wide, so a second concurrent test could distort it.
#[test]
fn bad_models_are_rejected_without_reserving_the_announced_tables() {
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    let header = hostile_header();
    cases.push(("hostile header without data".into(), header.clone()));
    let mut short_data = header;
    short_data.extend_from_slice(&[0; 1000]);
    cases.push(("hostile header with a little data".into(), short_data));
    for cut in [200, REAL_MODEL.len() / 2, REAL_MODEL.len() - 1] {
        cases.push((
            format!("real model cut to {cut} bytes"),
            REAL_MODEL[..cut].to_vec(),
        ));
    }
    let mut oversized = REAL_MODEL.to_vec();
    oversized.push(0);
    cases.push(("real model with a trailing byte".into(), oversized));

    for (name, bytes) in &cases {
        let largest = rejected_with_largest_allocation(bytes);
        assert!(
            largest < FAILED_PARSE_ALLOCATION_LIMIT,
            "{name}: rejecting it reserved {largest} bytes"
        );
    }
    // The genuine model still parses.
    assert!(LayoutModel::parse(REAL_MODEL).is_ok());
}
