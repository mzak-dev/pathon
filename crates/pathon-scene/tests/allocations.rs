// Steady-state extracts must not allocate: the buffers keep their capacity and the swap moves no memory. The counter is
// per thread, so the test harness's own threads do not disturb it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use pathon_api::glam::{Affine3A, Vec3};
use pathon_api::rhi::{BufferId, Id, RawHandle};
use pathon_scene::{InstanceDesc, InstanceId, MaterialHandle, MeshHandle, Scene};

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: every call is forwarded unchanged to the system allocator, which upholds the `GlobalAlloc` contract.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` come from a matching `alloc` of this allocator, i.e. of `System`.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: the caller's contract for `realloc` is passed through.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations_during(action: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    action();
    ALLOCATIONS.with(Cell::get) - before
}

fn at(x: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, 0.0, 0.0))
}

fn move_all(scene: &mut Scene, ids: &[InstanceId], offset: f32) {
    scene.extract(|writer| {
        for (index, id) in ids.iter().enumerate() {
            writer.set_transform(*id, at(index as f32 + offset)).expect("live");
        }
    });
}

#[test]
fn steady_state_extracts_do_not_allocate() {
    const COUNT: usize = 10_000;
    let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
    let mut scene = Scene::new();
    let ids: Vec<InstanceId> = scene.extract(|writer| {
        (0..COUNT)
            .map(|index| {
                writer
                    .spawn(&InstanceDesc::new(mesh, MaterialHandle::DEFAULT, at(index as f32)))
                    .expect("finite")
            })
            .collect()
    });
    // Both buffers have to grow their change lists once.
    for step in 1..=4 {
        move_all(&mut scene, &ids, step as f32);
    }

    let moving = allocations_during(|| move_all(&mut scene, &ids, 10.0));
    let moving_again = allocations_during(|| move_all(&mut scene, &ids, 11.0));
    let idle = allocations_during(|| move_all(&mut scene, &ids, 11.0));
    let snapshot_and_read = allocations_during(|| {
        let world = scene.snapshot();
        assert_eq!(world.instances().len(), COUNT);
    });

    assert_eq!((moving, moving_again, idle, snapshot_and_read), (0, 0, 0, 0));
}
