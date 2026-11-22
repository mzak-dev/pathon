use std::collections::HashSet;

use pathon_mesh::{
    ArenaKind, DeformedPositions, GeometryPool, IndexData, MeshData, MeshError, MeshId, PoolDesc, VertexLayout,
};
use pathon_rhi::{
    BufferDesc, BufferId, BufferUsage, Command, CommandList, Device, DeviceApi, IndexFormat, RenderPassDesc,
};

// A mesh whose vertices carry their own identity: position = [index, tag, 0].
struct TestMesh {
    positions: Vec<[f32; 3]>,
    surface: Vec<u8>,
    indices: Vec<u32>,
}

impl TestMesh {
    fn new(tag: f32, vertices: u32, indices: u32) -> Self {
        Self {
            positions: (0..vertices).map(|index| [index as f32, tag, 0.0]).collect(),
            surface: vec![0; vertices as usize * 36],
            indices: (0..indices).map(|index| index % vertices).collect(),
        }
    }

    fn data(&self, label: &'static str) -> MeshData<'_> {
        MeshData::new(
            label,
            vec![bytemuck::cast_slice(&self.positions), &self.surface],
            IndexData::U32(&self.indices),
        )
    }
}

fn pool_with(device: &Device, chunk: (u32, u32), budget: (u64, u64)) -> GeometryPool {
    GeometryPool::new(
        device,
        "test pool",
        VertexLayout::standard(),
        PoolDesc::default()
            .with_chunk_sizes(chunk.0, chunk.1)
            .with_budget(budget.0, budget.1)
            .with_frames_in_flight(2),
    )
}

fn read(device: &Device, buffer: BufferId, offset: u64, size: u64) -> Vec<u8> {
    let readback = device
        .create_buffer(&BufferDesc {
            label: "readback",
            size,
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback");
    let mut list = CommandList::new();
    list.copy_buffer_to_buffer(buffer, offset, readback, 0, size);
    device.submit(list).expect("copy");
    device.read_buffer(readback).expect("read")
}

fn read_positions(device: &Device, pool: &GeometryPool, mesh: MeshId) -> Vec<[f32; 3]> {
    let location = pool.mesh(mesh).expect("live").location;
    let buffer = pool.vertex_buffers(location.vertex_chunk).expect("chunk")[0];
    let bytes = read(
        device,
        buffer,
        u64::from(location.base_vertex) * 12,
        u64::from(location.vertex_count) * 12,
    );
    bytemuck::pod_collect_to_vec(&bytes)
}

fn read_indices(device: &Device, pool: &GeometryPool, mesh: MeshId) -> Vec<u32> {
    let location = pool.mesh(mesh).expect("live").location;
    let buffer = pool.index_buffer(location.index_chunk).expect("chunk");
    let bytes = read(
        device,
        buffer,
        u64::from(location.first_index) * 4,
        u64::from(location.index_count) * 4,
    );
    bytemuck::pod_collect_to_vec(&bytes)
}

mod placement {
    use super::*;

    #[test]
    fn many_meshes_share_one_buffer_and_their_data_lands_where_reported() {
        let device = Device::null();
        let mut pool = pool_with(&device, (64, 96), (64, 96));
        let meshes = [
            TestMesh::new(1.0, 3, 3),
            TestMesh::new(2.0, 4, 6),
            TestMesh::new(3.0, 5, 9),
        ];
        let ids: Vec<MeshId> = meshes
            .iter()
            .map(|mesh| pool.upload(&device, &mesh.data("mesh")).expect("upload"))
            .collect();

        let locations: Vec<_> = ids.iter().map(|id| pool.mesh(*id).expect("live").location).collect();
        assert!(
            locations
                .iter()
                .all(|location| location.vertex_chunk == 0 && location.index_chunk == 0)
        );
        assert_eq!(pool.stats().vertices.chunks, 1);
        assert_eq!(pool.stats().indices.chunks, 1);
        assert_eq!(
            locations
                .iter()
                .map(|location| location.base_vertex)
                .collect::<Vec<_>>(),
            [0, 3, 7]
        );
        assert_eq!(
            locations
                .iter()
                .map(|location| location.first_index)
                .collect::<Vec<_>>(),
            [0, 3, 9]
        );
        for (id, mesh) in ids.iter().zip(&meshes) {
            assert_eq!(read_positions(&device, &pool, *id), mesh.positions);
            assert_eq!(read_indices(&device, &pool, *id), mesh.indices);
        }
        assert_eq!(pool.stats().meshes, 3);
    }

    #[test]
    fn sixteen_bit_indices_are_widened_and_bounds_are_computed() {
        let device = Device::null();
        let mut pool = pool_with(&device, (16, 16), (16, 16));
        let mesh = TestMesh::new(4.0, 4, 0);
        let indices: [u16; 6] = [0, 1, 2, 2, 1, 3];
        let data = MeshData::new(
            "quad",
            vec![bytemuck::cast_slice(&mesh.positions), &mesh.surface],
            IndexData::U16(&indices),
        );
        let id = pool.upload(&device, &data).expect("upload");

        assert_eq!(read_indices(&device, &pool, id), [0, 1, 2, 2, 1, 3]);
        let bounds = pool.mesh(id).expect("live").bounds;
        assert_eq!(
            (bounds.min.to_array(), bounds.max.to_array()),
            ([0.0, 4.0, 0.0], [3.0, 4.0, 0.0])
        );
    }

    #[test]
    fn ranges_of_live_meshes_never_overlap_under_churn() {
        let device = Device::null();
        let mut pool = pool_with(&device, (40, 60), (160, 240));
        let mut live: Vec<MeshId> = Vec::new();
        let mut seed = 11u64;
        for frame in 0..300u64 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            pool.begin_frame(&device, frame);
            if seed & (1 << 22) != 0 && !live.is_empty() {
                let index = usize::try_from(seed >> 40).expect("fits") % live.len();
                pool.remove(live.swap_remove(index), frame).expect("remove");
            } else {
                let vertices = u32::try_from(seed >> 59).expect("fits") + 3;
                let mesh = TestMesh::new(frame as f32, vertices, vertices);
                if let Ok(id) = pool.upload(&device, &mesh.data("churn")) {
                    live.push(id);
                }
            }
            let mut claimed: HashSet<(u32, u32)> = HashSet::new();
            let mut index_claimed: HashSet<(u32, u32)> = HashSet::new();
            for id in &live {
                let location = pool.mesh(*id).expect("live").location;
                for vertex in location.base_vertex..location.base_vertex + location.vertex_count {
                    assert!(
                        claimed.insert((location.vertex_chunk, vertex)),
                        "vertex {vertex} claimed twice"
                    );
                }
                for index in location.indices() {
                    assert!(
                        index_claimed.insert((location.index_chunk, index)),
                        "index {index} claimed twice"
                    );
                }
            }
        }
        for id in &live {
            let positions = read_positions(&device, &pool, *id);
            assert!(
                positions
                    .windows(2)
                    .all(|pair| pair[0][1].to_bits() == pair[1][1].to_bits()),
                "mesh data was overwritten"
            );
        }
    }
}

mod freeing_and_reuse {
    use super::*;

    #[test]
    fn a_removed_range_comes_back_only_after_the_frames_in_flight() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        let first = pool
            .upload(&device, &TestMesh::new(1.0, 8, 12).data("first"))
            .expect("first");
        let location = pool.mesh(first).expect("live").location;

        pool.remove(first, 5).expect("remove");
        assert!(pool.mesh(first).is_none(), "the handle dies at once");
        assert_eq!(pool.stats().vertices.pending, 8);
        assert_eq!(pool.begin_frame(&device, 6), 0);
        let blocked = pool
            .upload(&device, &TestMesh::new(2.0, 1, 3).data("blocked"))
            .expect_err("still pending");
        assert_eq!(blocked.code(), "M0003");

        assert_eq!(pool.begin_frame(&device, 7), 1);
        assert_eq!(pool.stats().vertices.pending, 0);
        let second = pool
            .upload(&device, &TestMesh::new(3.0, 8, 12).data("second"))
            .expect("reuse");
        assert_eq!(pool.mesh(second).expect("live").location, location);
        assert_eq!(read_positions(&device, &pool, second)[0][1], 3.0);
    }

    #[test]
    fn a_stale_handle_never_reaches_the_reused_slot() {
        let device = Device::null();
        let mut pool = pool_with(&device, (16, 16), (16, 16));
        let old = pool
            .upload(&device, &TestMesh::new(1.0, 3, 3).data("old"))
            .expect("old");
        pool.remove(old, 0).expect("remove");
        pool.begin_frame(&device, 2);
        let new = pool
            .upload(&device, &TestMesh::new(2.0, 3, 3).data("new"))
            .expect("new");

        assert_ne!(old, new);
        assert!(pool.mesh(old).is_none());
        assert_eq!(pool.remove(old, 3), Err(MeshError::UnknownMesh));
        assert!(pool.mesh(new).is_some());
    }

    #[test]
    fn an_empty_extra_chunk_gives_its_buffers_back() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (32, 48));
        let a = pool.upload(&device, &TestMesh::new(1.0, 8, 12).data("a")).expect("a");
        let b = pool.upload(&device, &TestMesh::new(2.0, 8, 12).data("b")).expect("b");
        let second_chunk_buffer = pool.vertex_buffers(1).expect("second chunk")[0];
        assert_eq!(pool.stats().vertices.chunks, 2);

        pool.remove(b, 0).expect("remove");
        pool.begin_frame(&device, 2);

        assert_eq!(pool.stats().vertices.chunks, 1);
        assert!(pool.vertex_buffers(1).is_none());
        assert!(
            device.write_buffer(second_chunk_buffer, 0, &[0; 4]).is_err(),
            "buffer destroyed"
        );
        assert!(pool.mesh(a).is_some());
    }

    #[test]
    fn forget_gpu_invalidates_every_handle_and_the_pool_is_usable_again() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        let id = pool.upload(&device, &TestMesh::new(1.0, 8, 12).data("a")).expect("a");
        pool.remove(id, 0).expect("remove");
        let other = pool.upload(&device, &TestMesh::new(1.0, 0, 0).data("empty"));
        assert!(other.is_err());

        pool.forget_gpu();

        assert_eq!(pool.stats().meshes, 0);
        assert_eq!(pool.stats().vertices.capacity, 0);
        assert!(pool.upload(&device, &TestMesh::new(2.0, 8, 12).data("again")).is_ok());
    }

    #[test]
    fn release_all_destroys_the_buffers() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        let id = pool.upload(&device, &TestMesh::new(1.0, 4, 6).data("a")).expect("a");
        let location = pool.mesh(id).expect("live").location;
        let vertex_buffer = pool.vertex_buffers(location.vertex_chunk).expect("chunk")[0];
        let index_buffer = pool.index_buffer(location.index_chunk).expect("chunk");

        pool.release_all(&device);

        assert!(device.write_buffer(vertex_buffer, 0, &[0; 4]).is_err());
        assert!(device.write_buffer(index_buffer, 0, &[0; 4]).is_err());
        assert!(pool.mesh(id).is_none());
    }
}

mod limits_and_diagnostics {
    use super::*;

    #[test]
    fn a_mesh_exactly_as_big_as_a_chunk_fits_and_one_vertex_more_gets_its_own_chunk() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (32, 48));
        let exact = pool
            .upload(&device, &TestMesh::new(1.0, 8, 12).data("exact"))
            .expect("exact");
        let larger = pool
            .upload(&device, &TestMesh::new(2.0, 9, 12).data("larger"))
            .expect("larger");

        assert_eq!(pool.mesh(exact).expect("live").location.vertex_chunk, 0);
        let location = pool.mesh(larger).expect("live").location;
        assert_eq!((location.vertex_chunk, location.base_vertex), (1, 0));
        assert_eq!(pool.stats().vertices.capacity, 8 + 9);
        pool.remove(larger, 0).expect("remove");
        pool.begin_frame(&device, 2);
        assert_eq!(pool.stats().vertices.chunks, 1, "a dedicated chunk is released at once");
    }

    #[test]
    fn a_mesh_larger_than_the_whole_budget_is_too_large() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (16, 24));

        let error = pool
            .upload(&device, &TestMesh::new(1.0, 17, 3).data("huge"))
            .expect_err("too large");
        assert!(matches!(
            error,
            MeshError::TooLarge {
                kind: ArenaKind::Vertices,
                requested: 17,
                max: 16,
                ..
            }
        ));
        assert_eq!(error.code(), "M0005");

        let error = pool
            .upload(&device, &TestMesh::new(1.0, 3, 25).data("many indices"))
            .expect_err("too large");
        assert!(matches!(
            error,
            MeshError::TooLarge {
                kind: ArenaKind::Indices,
                ..
            }
        ));
        assert_eq!(
            pool.stats().vertices.used,
            0,
            "the vertex range of the failed upload was given back"
        );
    }

    #[test]
    fn exhausting_the_budget_reports_m0003_with_a_hint() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        pool.upload(&device, &TestMesh::new(1.0, 5, 6).data("a")).expect("a");
        pool.upload(&device, &TestMesh::new(2.0, 3, 6).data("b")).expect("b");

        let error = pool
            .upload(&device, &TestMesh::new(3.0, 1, 3).data("c"))
            .expect_err("exhausted");
        assert!(matches!(
            error,
            MeshError::Exhausted {
                kind: ArenaKind::Vertices,
                requested: 1,
                free: 0,
                ..
            }
        ));
        let diagnostic = error.to_diagnostic();
        assert_eq!(diagnostic.code, "M0003");
        assert!(diagnostic.hint.is_some());
        assert_eq!(pool.stats().indices.used, 12, "the failed upload took nothing");
    }

    #[test]
    fn holes_that_cannot_fit_a_mesh_report_fragmentation_not_exhaustion() {
        let device = Device::null();
        let mut pool = pool_with(&device, (12, 36), (12, 36));
        let ids: Vec<MeshId> = (0..3)
            .map(|index| {
                pool.upload(&device, &TestMesh::new(index as f32, 4, 6).data("slice"))
                    .expect("slice")
            })
            .collect();
        pool.remove(ids[0], 0).expect("remove");
        pool.remove(ids[2], 0).expect("remove");
        pool.begin_frame(&device, 2);

        let error = pool
            .upload(&device, &TestMesh::new(9.0, 6, 6).data("wide"))
            .expect_err("fragmented");
        assert!(matches!(
            error,
            MeshError::Fragmented {
                kind: ArenaKind::Vertices,
                requested: 6,
                free: 8,
                largest_free: 4,
                ..
            }
        ));
        assert_eq!(error.code(), "M0004");
        let stats = pool.stats().vertices;
        assert_eq!((stats.free, stats.largest_free), (8, 4));
        assert!(stats.fragmentation() > 0.4);

        pool.remove(ids[1], 2).expect("remove");
        pool.begin_frame(&device, 4);
        assert!(pool.upload(&device, &TestMesh::new(9.0, 6, 6).data("wide")).is_ok());
    }

    #[test]
    fn malformed_meshes_never_touch_the_pool() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        let mesh = TestMesh::new(1.0, 3, 3);
        let wrong_layout = MeshData::new(
            "one stream",
            vec![bytemuck::cast_slice(&mesh.positions)],
            IndexData::U32(&mesh.indices),
        );

        assert_eq!(pool.upload(&device, &wrong_layout).expect_err("layout").code(), "M0002");
        assert_eq!(
            pool.upload(&device, &TestMesh::new(1.0, 0, 0).data("empty"))
                .expect_err("empty")
                .code(),
            "M0002"
        );
        assert_eq!(pool.stats(), pool_with(&device, (8, 12), (8, 12)).stats());
    }

    #[test]
    fn a_failed_device_call_rolls_the_allocation_back() {
        let device = Device::null();
        let mut pool = pool_with(&device, (8, 12), (8, 12));
        device.as_null().expect("null device").simulate_device_loss();

        let error = pool
            .upload(&device, &TestMesh::new(1.0, 3, 3).data("lost"))
            .expect_err("device lost");

        assert_eq!(error.code(), "R0006");
        assert_eq!(pool.stats().vertices.chunks, 0);
        assert_eq!(pool.stats().vertices.used, 0);
        assert_eq!(pool.stats().meshes, 0);
    }
}

mod recording {
    use super::*;

    #[test]
    fn draw_binds_every_stream_and_passes_the_base_vertex() {
        let device = Device::null();
        let mut pool = pool_with(&device, (16, 24), (16, 24));
        pool.upload(&device, &TestMesh::new(1.0, 3, 3).data("first"))
            .expect("first");
        let second = pool
            .upload(&device, &TestMesh::new(2.0, 4, 6).data("second"))
            .expect("second");
        let mut list = CommandList::new();
        {
            let mut pass = list.render_pass(RenderPassDesc {
                label: "draw",
                color: vec![],
                depth: None,
                timestamps: None,
            });
            pool.draw(&mut pass, second, 0..2).expect("draw");
        }

        let commands = list.commands();
        let slots: Vec<u32> = commands
            .iter()
            .filter_map(|command| match command {
                Command::SetVertexBuffer { slot, .. } => Some(*slot),
                _ => None,
            })
            .collect();
        assert_eq!(slots, [0, 1]);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::SetIndexBuffer {
                format: IndexFormat::Uint32,
                offset: 0,
                ..
            }
        )));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::DrawIndexed { indices, base_vertex: 3, instances } if *indices == (3..9) && *instances == (0..2)
        )));
    }

    #[test]
    fn drawing_a_removed_mesh_is_m0006() {
        let device = Device::null();
        let mut pool = pool_with(&device, (16, 24), (16, 24));
        let id = pool
            .upload(&device, &TestMesh::new(1.0, 3, 3).data("gone"))
            .expect("upload");
        pool.remove(id, 0).expect("remove");
        let mut list = CommandList::new();
        let mut pass = list.render_pass(RenderPassDesc {
            label: "draw",
            color: vec![],
            depth: None,
            timestamps: None,
        });

        assert_eq!(pool.draw(&mut pass, id, 0..1).expect_err("removed").code(), "M0006");
    }
}

mod deformed_positions {
    use super::*;

    #[test]
    fn ranges_are_valid_in_both_banks_and_offsets_follow_the_frame_parity() {
        let device = Device::null();
        let mut bank = DeformedPositions::new(&device, "deformed", 100, 2).expect("bank");
        let first = bank.allocate(10).expect("first");
        let second = bank.allocate(20).expect("second");

        assert_eq!((first.first_vertex, second.first_vertex), (0, 10));
        assert_eq!(bank.current_offset(second, 4), 10 * 16);
        assert_eq!(bank.previous_offset(second, 4), (100 + 10) * 16);
        assert_eq!(bank.current_offset(second, 5), bank.previous_offset(second, 4));
        assert_eq!(bank.previous_offset(second, 5), bank.current_offset(second, 4));
    }

    #[test]
    fn freed_ranges_wait_for_frames_in_flight_and_exhaustion_has_a_code() {
        let device = Device::null();
        let mut bank = DeformedPositions::new(&device, "deformed", 30, 2).expect("bank");
        let a = bank.allocate(30).expect("all");
        assert_eq!(bank.allocate(1).expect_err("full").code(), "M0003");

        bank.free(a, 3);
        bank.begin_frame(4);
        assert_eq!(bank.allocate(1).expect_err("pending").code(), "M0003");
        bank.begin_frame(5);
        assert_eq!(bank.allocate(30).expect("reuse").first_vertex, 0);
        assert_eq!(bank.allocate(0).expect_err("empty").code(), "M0002");
    }

    #[test]
    fn a_bank_beyond_the_buffer_limit_is_too_large() {
        let device = Device::null();

        let error = DeformedPositions::new(&device, "deformed", u32::MAX, 2).expect_err("too large");

        assert_eq!(error.code(), "M0005");
    }

    #[test]
    fn the_buffer_serves_storage_and_vertex_use() {
        let device = Device::null();
        let bank = DeformedPositions::new(&device, "deformed", 4, 2).expect("bank");
        let buffer = bank.buffer();

        assert!(
            device.write_buffer(buffer, 0, &[0; 128]).is_ok(),
            "two banks of four vec4"
        );
        bank.release(&device);
        assert!(device.write_buffer(buffer, 0, &[0; 4]).is_err());
    }
}

mod transform_history_on_the_gpu {
    use pathon_api::glam::{Mat4, Vec3};
    use pathon_api::resources::DeferredRelease;
    use pathon_mesh::{AffineRows, ObjectTransforms, TransformHistory};

    use super::*;

    #[test]
    fn the_uploaded_buffer_holds_model_and_previous_model_side_by_side() {
        let device = Device::null();
        let mut release = DeferredRelease::new(2);
        let mut history = TransformHistory::new("transforms");
        let move_x = |x: f32| AffineRows::from_mat4(Mat4::from_translation(Vec3::new(x, 0.0, 0.0)));
        let moving = history.push(move_x(0.0));
        let still = history.push(move_x(7.0));

        history.begin_frame(1);
        history.set(moving, move_x(2.0));
        let buffer = history.sync(&device, &mut release, 1).expect("sync");

        let bytes = read(&device, buffer, 0, 2 * 96);
        let objects: Vec<ObjectTransforms> = bytemuck::pod_collect_to_vec(&bytes);
        assert_eq!(objects[moving], *history.get(moving).expect("object"));
        assert_eq!(
            (objects[moving].model, objects[moving].previous_model),
            (move_x(2.0), move_x(0.0))
        );
        assert_eq!(
            (objects[still].model, objects[still].previous_model),
            (move_x(7.0), move_x(7.0))
        );
    }
}
