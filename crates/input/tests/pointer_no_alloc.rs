//! Pointer recording and overflow resynchronisation remain allocation-free after mapper setup.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ck_input::{Bindings, Mapper, PointerEvent, PointerPosition, ScrollUnit};

thread_local! {
    static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: this allocator forwards every allocation operation unchanged to `System`; its counters are thread-local.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: forward the caller's layout unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forward the allocation and its original layout unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: forward the allocation, its original layout, and requested size unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: forward the caller's layout unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn pointer_event_methods_allocate_nothing_after_mapper_construction() {
    let bindings = Bindings::from_entries([]).expect("empty bindings");
    let mut mapper = Mapper::new(bindings);
    // Warm the thread-local counters before measuring the event methods.
    let before = ALLOCATIONS.with(Cell::get);
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    mapper.pointer(PointerEvent::Down(PointerPosition { x: 1, y: 2 }));
    mapper.pointer(PointerEvent::Move(PointerPosition { x: 3, y: 4 }));
    mapper.pointer(PointerEvent::Scroll {
        position: PointerPosition { x: 3, y: 4 },
        amount: 128,
        unit: ScrollUnit::Lines,
    });
    mapper.pointer_overflow(PointerPosition { x: 5, y: 6 }, true);
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    let allocated = ALLOCATIONS.with(Cell::get) - before;
    assert_eq!(
        allocated, 0,
        "pointer recording and overflow resync use fixed storage"
    );
}
