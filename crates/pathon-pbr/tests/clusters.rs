// Light assignment against a CPU brute force on random scenes (up to 1000 lights, perspective and orthographic). The
// null half validates every command and binding everywhere; the GPU half (`--features gpu-tests`) reads the lists back.

mod support;

use pathon_api::glam::{Mat4, Vec3};
use pathon_api::rhi::{BindingResource, PhysicalSize};
use pathon_material::MaterialHooks;
use pathon_pbr::reference::{SphereLight, assign_lights};
use pathon_pbr::{
    COUNTERS_SIZE, ClusterBuffers, ClusterGrid, ClusterParams, ClusterPipeline, ClusterSettings, GpuLight, PbrShaders,
    RANGE_SIZE,
};
use pathon_rhi::{BufferUsage, CommandList, DeviceApi, NullDevice};
use pathon_scene::Projection;
use support::{Lcg, buffer};

const SIZE: PhysicalSize = PhysicalSize::new(128, 96);

struct Scene {
    projection: Mat4,
    world_to_view: Mat4,
    lights: Vec<SphereLight>,
    settings: ClusterSettings,
}

fn camera(orthographic: bool) -> (Mat4, Mat4) {
    let aspect = SIZE.width as f32 / SIZE.height as f32;
    let projection = if orthographic {
        Projection::orthographic(12.0, 0.1, 80.0)
    } else {
        Projection::perspective(std::f32::consts::FRAC_PI_3, 0.1)
    };
    let world_to_view = pathon_api::glam::camera::rh::view::look_at_mat4(
        Vec3::new(1.0, 2.0, 8.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::Y,
    );
    (projection.matrix(aspect), world_to_view)
}

fn random_scene(seed: u64, lights: usize, orthographic: bool) -> Scene {
    let mut random = Lcg(seed);
    let (projection, world_to_view) = camera(orthographic);
    let lights = (0..lights)
        .map(|_| SphereLight {
            position: Vec3::new(random.between(-14.0, 14.0), random.between(-1.0, 8.0), random.between(-30.0, 10.0)),
            range: random.between(0.5, 5.0),
        })
        .collect();
    Scene {
        projection,
        world_to_view,
        lights,
        settings: ClusterSettings::default(),
    }
}

struct Assignment {
    ranges: Vec<[u32; 2]>,
    indices: Vec<u32>,
    counters: [u32; 2],
}

fn read(device: &dyn DeviceApi, buffer_id: pathon_rhi::BufferId, size: u64) -> Vec<u8> {
    let readback = buffer(device, "readback", size, BufferUsage::MAP_READ | BufferUsage::COPY_DST);
    let mut list = CommandList::new();
    list.buffer_access(buffer_id, pathon_rhi::Access::COPY_SRC);
    list.copy_buffer_to_buffer(buffer_id, 0, readback, 0, size);
    device.submit(list).expect("copy");
    device.read_buffer(readback).expect("read")
}

fn run(device: &dyn DeviceApi, scene: &Scene) -> (ClusterGrid, Assignment) {
    let mut shaders = PbrShaders::new(&MaterialHooks::default()).expect("shaders");
    let pipeline = ClusterPipeline::new(device, &mut shaders).expect("pipelines");
    let grid = ClusterGrid::new(SIZE, &scene.settings);
    let gpu_lights: Vec<GpuLight> = scene
        .lights
        .iter()
        .map(|light| GpuLight {
            position_range: [light.position.x, light.position.y, light.position.z, light.range],
            direction_kind: [0.0, 0.0, -1.0, GpuLight::KIND_POINT],
            ..GpuLight::default()
        })
        .collect();
    let params = ClusterParams::new(
        &grid,
        scene.projection,
        scene.world_to_view,
        SIZE,
        0,
        u32::try_from(gpu_lights.len()).expect("few lights"),
        scene.settings.max_light_indices,
    );
    let storage = BufferUsage::STORAGE | BufferUsage::COPY_DST | BufferUsage::COPY_SRC;
    let params_buffer = buffer(device, "params", std::mem::size_of::<ClusterParams>() as u64, BufferUsage::UNIFORM | BufferUsage::COPY_DST);
    device.write_buffer(params_buffer, 0, bytemuck::bytes_of(&params)).expect("params");
    let lights_buffer = buffer(device, "lights", (gpu_lights.len().max(1) * 64) as u64, storage);
    if !gpu_lights.is_empty() {
        device.write_buffer(lights_buffer, 0, bytemuck::cast_slice(&gpu_lights)).expect("lights");
    }
    let ranges_size = u64::from(grid.count()) * RANGE_SIZE;
    let indices_size = u64::from(scene.settings.max_light_indices) * 4;
    let ranges = buffer(device, "ranges", ranges_size, storage);
    let indices = buffer(device, "indices", indices_size, storage);
    let counters = buffer(device, "counters", COUNTERS_SIZE, storage);

    let group = device
        .create_bind_group(&pipeline.bind_group_desc(&ClusterBuffers {
            params: BindingResource::whole_buffer(params_buffer),
            lights: BindingResource::whole_buffer(lights_buffer),
            ranges: BindingResource::whole_buffer(ranges),
            indices: BindingResource::whole_buffer(indices),
            counters: BindingResource::whole_buffer(counters),
        }))
        .expect("bind group");
    let mut list = CommandList::new();
    pipeline.record(&mut list, group, &grid);
    device.submit(list).expect("cluster pass validates");

    let ranges: Vec<[u32; 2]> = bytemuck::cast_slice(&read(device, ranges, ranges_size)).to_vec();
    let indices: Vec<u32> = bytemuck::cast_slice(&read(device, indices, indices_size)).to_vec();
    let counters: Vec<u32> = bytemuck::cast_slice(&read(device, counters, COUNTERS_SIZE)).to_vec();
    (
        grid,
        Assignment {
            ranges,
            indices,
            counters: [counters[0], counters[1]],
        },
    )
}

#[test]
fn the_cluster_pass_validates_on_the_null_device() {
    let device = NullDevice::new();
    for orthographic in [false, true] {
        let scene = random_scene(1, 40, orthographic);
        let (grid, assignment) = run(&device, &scene);
        assert_eq!(assignment.ranges.len(), grid.count() as usize);
    }
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use super::*;
    use support::gpu;

    // A light within `TOLERANCE` metres of a cluster's boundary may be on either side of float rounding.
    const TOLERANCE: f64 = 2.0e-3;

    fn list_of(assignment: &Assignment, cluster: usize) -> &[u32] {
        let [offset, count] = assignment.ranges[cluster];
        if count == 0 {
            return &[];
        }
        &assignment.indices[offset as usize..(offset + count) as usize]
    }

    fn check_against_brute_force(scene: &Scene, grid: &ClusterGrid, assignment: &Assignment) {
        let expected = assign_lights(grid, scene.projection, scene.world_to_view, SIZE, &scene.lights, TOLERANCE);
        let mut with_lights = 0;
        for (cluster, expectation) in expected.iter().enumerate() {
            let got = list_of(assignment, cluster);
            assert!(got.windows(2).all(|pair| pair[0] < pair[1]), "cluster {cluster}: not in light order");
            for must in &expectation.must {
                assert!(got.contains(must), "cluster {cluster} lacks light {must}: {got:?} vs {expectation:?}");
            }
            for light in got {
                assert!(
                    expectation.may.contains(light),
                    "cluster {cluster} has light {light} that does not touch it: {got:?} vs {expectation:?}"
                );
            }
            with_lights += usize::from(!got.is_empty());
        }
        assert!(
            scene.lights.len() < 100 || with_lights > expected.len() / 10,
            "the scene lights too few clusters to mean anything"
        );
    }

    #[test]
    fn random_scenes_match_brute_force_from_ten_to_a_thousand_lights() {
        let Some(device) = gpu("random_scenes_match_brute_force_from_ten_to_a_thousand_lights") else {
            return;
        };
        for (seed, lights, orthographic) in [(1, 10, false), (2, 100, false), (3, 1000, false), (4, 300, true), (5, 1000, true)] {
            let scene = random_scene(seed, lights, orthographic);
            let (grid, assignment) = run(&device, &scene);
            assert_eq!(assignment.counters[1], 0, "seed {seed}: nothing overflows the default list");
            check_against_brute_force(&scene, &grid, &assignment);
            let entries: usize = assignment.ranges.iter().map(|range| range[1] as usize).sum();
            assert_eq!(entries as u32, assignment.counters[0], "seed {seed}: the reservation counter matches the lists");
        }
    }

    #[test]
    fn a_point_gets_every_light_that_reaches_it() {
        let Some(device) = gpu("a_point_gets_every_light_that_reaches_it") else {
            return;
        };
        for orthographic in [false, true] {
            let scene = random_scene(9, 1000, orthographic);
            let (grid, assignment) = run(&device, &scene);
            let view_projection = scene.projection * scene.world_to_view;
            let mut random = Lcg(77);
            let mut checked = 0;
            while checked < 400 {
                let world = Vec3::new(random.between(-14.0, 14.0), random.between(-1.0, 8.0), random.between(-30.0, 10.0));
                let clip = view_projection * world.extend(1.0);
                let ndc = clip.truncate() / clip.w;
                let view_depth = -(scene.world_to_view * world.extend(1.0)).z;
                if clip.w <= 0.0 || ndc.x.abs() >= 1.0 || ndc.y.abs() >= 1.0 || view_depth <= 0.0 {
                    continue;
                }
                let pixel_x = (ndc.x * 0.5 + 0.5) * SIZE.width as f32;
                let pixel_y = (0.5 - ndc.y * 0.5) * SIZE.height as f32;
                let cluster = grid.index_of_pixel(pixel_x, pixel_y, view_depth) as usize;
                let list = list_of(&assignment, cluster);
                for (index, light) in scene.lights.iter().enumerate() {
                    if (light.position - world).length() < light.range - 0.01 {
                        assert!(
                            list.contains(&u32::try_from(index).expect("light index")),
                            "point {world} at pixel ({pixel_x}, {pixel_y}) depth {view_depth} misses light {index}"
                        );
                    }
                }
                checked += 1;
            }
        }
    }

    #[test]
    fn an_overflowing_list_is_counted_and_never_overruns() {
        let Some(device) = gpu("an_overflowing_list_is_counted_and_never_overruns") else {
            return;
        };
        let mut scene = random_scene(11, 300, false);
        scene.settings.max_light_indices = 512;

        let (grid, assignment) = run(&device, &scene);

        assert!(assignment.counters[1] > 0, "300 lights in 1152 clusters need more than 512 entries");
        let kept: u32 = assignment.ranges.iter().map(|range| range[1]).sum();
        assert!(kept <= 512, "{kept} entries kept in a list of 512");
        assert_eq!(kept + assignment.counters[1], assignment.counters[0], "every entry is either kept or counted");
        let expected = assign_lights(&grid, scene.projection, scene.world_to_view, SIZE, &scene.lights, TOLERANCE);
        for (cluster, expectation) in expected.iter().enumerate() {
            let [offset, _] = assignment.ranges[cluster];
            assert!(assignment.ranges[cluster][1] == 0 || offset + assignment.ranges[cluster][1] <= 512);
            for light in list_of(&assignment, cluster) {
                assert!(expectation.may.contains(light));
            }
        }
    }

    #[test]
    fn no_lights_means_empty_clusters() {
        let Some(device) = gpu("no_lights_means_empty_clusters") else {
            return;
        };
        let scene = random_scene(12, 0, false);

        let (_, assignment) = run(&device, &scene);

        assert!(assignment.ranges.iter().all(|range| range[1] == 0));
        assert_eq!(assignment.counters, [0, 0]);
    }
}
