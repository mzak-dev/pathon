use std::sync::atomic::{AtomicBool, Ordering};

use pathon_rhi::{
    ColorTargetState, ComputePipelineDesc, Device, Format, PrimitiveState, RenderPipelineDesc, ShaderStage,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexStepMode,
};
use pathon_rhi_types::{
    BindGroupDesc, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutId, BufferDesc, BufferId, Caps, CommandList,
    ComputePipelineId, DeviceApi, Extent3d, PipelineCacheData, QuerySetDesc, QuerySetId, RenderPipelineId, Resource,
    RhiError, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, SurfaceConfig, SurfaceFrame, SurfaceId,
    SurfaceTarget, TexelLayout, TextureCopy, TextureDesc, TextureId, TextureViewDesc, TextureViewId,
};
use pathon_shader::{PipelineCache, ShaderComposer, ShaderError, ShaderFeatures, ShaderKey, ShaderLibrary};

const COMPUTE: &str = "
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@if(wide) const FACTOR: u32 = 4u;
@if(!wide) const FACTOR: u32 = 1u;

@compute @workgroup_size(1)
fn main() {
    @if(shadows) { out[0] = 10u * FACTOR; }
    @else { out[0] = FACTOR; }
}
";

const RENDER: &str = "
@vertex fn vs(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
";

struct FlakyDevice {
    inner: Device,
    fail_next_module: AtomicBool,
    fail_next_pipeline: AtomicBool,
}

impl FlakyDevice {
    fn new() -> Self {
        Self {
            inner: Device::null(),
            fail_next_module: AtomicBool::new(false),
            fail_next_pipeline: AtomicBool::new(false),
        }
    }
}

fn injected_failure() -> RhiError {
    RhiError::Backend("injected".to_owned())
}

impl DeviceApi for FlakyDevice {
    fn caps(&self) -> &Caps {
        self.inner.caps()
    }
    fn is_lost(&self) -> bool {
        self.inner.is_lost()
    }
    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError> {
        self.inner.create_buffer(desc)
    }
    fn write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        self.inner.write_buffer(buffer, offset, data)
    }
    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError> {
        self.inner.create_texture(desc)
    }
    fn texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError> {
        self.inner.texture_desc(texture)
    }
    fn write_texture(
        &self,
        dst: &TextureCopy,
        data: &[u8],
        layout: TexelLayout,
        size: Extent3d,
    ) -> Result<(), RhiError> {
        self.inner.write_texture(dst, data, layout, size)
    }
    fn create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError> {
        self.inner.create_texture_view(texture, desc)
    }
    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerId, RhiError> {
        self.inner.create_sampler(desc)
    }
    fn create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError> {
        if self.fail_next_module.swap(false, Ordering::SeqCst) {
            return Err(injected_failure());
        }
        self.inner.create_shader_module(desc)
    }
    fn create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError> {
        self.inner.create_bind_group_layout(desc)
    }
    fn create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError> {
        self.inner.create_bind_group(desc)
    }
    fn create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError> {
        if self.fail_next_pipeline.swap(false, Ordering::SeqCst) {
            return Err(injected_failure());
        }
        self.inner.create_render_pipeline(desc)
    }
    fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError> {
        if self.fail_next_pipeline.swap(false, Ordering::SeqCst) {
            return Err(injected_failure());
        }
        self.inner.create_compute_pipeline(desc)
    }
    fn create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError> {
        self.inner.create_query_set(desc)
    }
    fn destroy(&self, resource: Resource) {
        self.inner.destroy(resource);
    }
    fn create_surface(&self, target: &SurfaceTarget) -> Result<SurfaceId, RhiError> {
        self.inner.create_surface(target)
    }
    fn configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<Format, RhiError> {
        self.inner.configure_surface(surface, config)
    }
    fn acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError> {
        self.inner.acquire_surface(surface)
    }
    fn present_surface(&self, surface: SurfaceId) -> Result<(), RhiError> {
        self.inner.present_surface(surface)
    }
    fn discard_surface(&self, surface: SurfaceId) {
        self.inner.discard_surface(surface);
    }
    fn destroy_surface(&self, surface: SurfaceId) {
        self.inner.destroy_surface(surface);
    }
    fn submit(&self, commands: CommandList) -> Result<(), RhiError> {
        self.inner.submit(commands)
    }
    fn read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError> {
        self.inner.read_buffer(buffer)
    }
    fn wait_idle(&self) -> Result<(), RhiError> {
        self.inner.wait_idle()
    }
    fn pipeline_cache_data(&self) -> Option<PipelineCacheData> {
        self.inner.pipeline_cache_data()
    }
}

fn features(names: &[&str]) -> ShaderFeatures {
    names.iter().fold(ShaderFeatures::default(), |features, name| {
        features.with(name).expect("flag")
    })
}

fn key(root: &str, names: &[&str]) -> ShaderKey {
    ShaderKey {
        root: root.to_owned(),
        features: features(names),
    }
}

fn library() -> ShaderLibrary {
    let mut library = ShaderLibrary::new();
    library.add_root("compute", COMPUTE);
    library.add_root("render", RENDER);
    library
}

mod features_and_composing {
    use super::*;

    #[test]
    fn an_enabled_flag_selects_the_declaration_and_the_statement_branch() {
        let composer = ShaderComposer::new();
        let plain = composer.compose(COMPUTE, "compute.wgsl").expect("plain");
        assert!(plain.contains("const FACTOR: u32 = 1u"), "{plain}");
        assert!(!plain.contains("10u"), "{plain}");

        let both = composer
            .compose_with(COMPUTE, "compute.wgsl", &features(&["wide", "shadows"]))
            .expect("both");
        assert!(both.contains("const FACTOR: u32 = 4u"), "{both}");
        assert!(both.contains("10u * FACTOR"), "{both}");
        assert!(!both.contains("1u;"), "{both}");
    }

    #[test]
    fn compose_without_flags_equals_compose_with_empty_features() {
        let composer = ShaderComposer::new();
        assert_eq!(
            composer.compose(COMPUTE, "c.wgsl").expect("a"),
            composer
                .compose_with(COMPUTE, "c.wgsl", &ShaderFeatures::default())
                .expect("b")
        );
    }

    #[test]
    fn flags_reach_module_code_too() {
        let mut composer = ShaderComposer::new();
        composer
            .add_module(
                "mode",
                "@if(fast) fn mode() -> u32 { return 1u; }\n@if(!fast) fn mode() -> u32 { return 2u; }",
            )
            .expect("module");
        let source = "import package::mode::mode;\n@compute @workgroup_size(1) fn main() { _ = mode(); }";
        let fast = composer
            .compose_with(source, "s.wgsl", &features(&["fast"]))
            .expect("fast");
        assert!(fast.contains("return 1u"), "{fast}");
        let slow = composer.compose(source, "s.wgsl").expect("slow");
        assert!(slow.contains("return 2u"), "{slow}");
    }

    #[test]
    fn flag_sets_ignore_order_duplicates_and_explicit_disabling() {
        let mut shuffled = features(&["b", "a"]);
        shuffled.set("a", true).expect("a");
        shuffled.set("c", true).expect("c");
        shuffled.set("c", false).expect("c off");
        assert_eq!(shuffled, features(&["a", "b"]));
        assert_eq!(shuffled.iter().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn flag_names_must_be_wgsl_identifiers() {
        for name in ["", "two words", "1abc", "a::b", "a-b", "_", "fn", "if", "true", "false"] {
            let error = ShaderFeatures::default().with(name).expect_err(name);
            assert!(
                matches!(error, ShaderError::InvalidFeatureName { .. }),
                "{name:?}: {error:?}"
            );
        }
        assert!(ShaderFeatures::default().set("bad name", true).is_err());
    }
}

mod shader_library {
    use super::*;

    #[test]
    fn the_same_permutation_gives_one_module_whatever_the_flag_order() {
        let device = Device::null();
        let mut library = library();
        let first = library
            .module(&device, &key("compute", &["wide", "shadows"]))
            .expect("first");
        let second = library
            .module(&device, &key("compute", &["shadows", "wide"]))
            .expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn an_explicitly_disabled_flag_is_the_same_permutation_as_an_absent_one() {
        let device = Device::null();
        let mut library = library();
        let absent = library.module(&device, &key("compute", &["wide"])).expect("absent");
        let mut disabled = features(&["wide", "shadows"]);
        disabled.set("shadows", false).expect("off");
        let explicit = library
            .module(
                &device,
                &ShaderKey {
                    root: "compute".to_owned(),
                    features: disabled,
                },
            )
            .expect("explicit");
        assert_eq!(absent, explicit);
    }

    #[test]
    fn different_flags_or_roots_give_different_modules() {
        let device = Device::null();
        let mut library = library();
        let none = library.module(&device, &key("compute", &[])).expect("none");
        let wide = library.module(&device, &key("compute", &["wide"])).expect("wide");
        let shadows = library.module(&device, &key("compute", &["shadows"])).expect("shadows");
        let render = library.module(&device, &key("render", &[])).expect("render");
        let all = [none, wide, shadows, render];
        for (index, id) in all.iter().enumerate() {
            assert!(!all[..index].contains(id), "module {index} repeats an earlier one");
        }
    }

    #[test]
    fn an_unknown_root_is_an_error() {
        let error = library()
            .module(&Device::null(), &key("missing", &[]))
            .expect_err("unknown");
        assert!(
            matches!(error, ShaderError::UnknownRoot { ref name } if name == "missing"),
            "{error:?}"
        );
    }

    #[test]
    fn a_composing_error_is_returned_every_time_and_never_cached() {
        let device = Device::null();
        let mut library = ShaderLibrary::new();
        library.add_root("broken", "fn (");
        for _ in 0..2 {
            let error = library.module(&device, &key("broken", &[])).expect_err("broken");
            assert!(matches!(error, ShaderError::Compose { .. }), "{error:?}");
        }
        library.add_root("broken", RENDER);
        library.module(&device, &key("broken", &[])).expect("fixed module");
    }

    fn no_evictions() -> Vec<pathon_rhi::ShaderModuleId> {
        Vec::new()
    }

    fn library_with_answer_module(device: &Device) -> (ShaderLibrary, pathon_rhi::ShaderModuleId) {
        let mut library = ShaderLibrary::new();
        library
            .add_module("answer", "fn answer() -> u32 { return 1u; }")
            .expect("module");
        library.add_root(
            "main",
            "import package::answer::answer;\n@compute @workgroup_size(1) fn main() { _ = answer(); }",
        );
        let id = library.module(device, &key("main", &[])).expect("module");
        (library, id)
    }

    #[test]
    fn registering_a_new_module_or_root_keeps_the_cached_module() {
        let device = Device::null();
        let (mut library, id) = library_with_answer_module(&device);
        assert_eq!(
            library.add_module("unrelated", "fn f() {}").expect("module"),
            no_evictions()
        );
        assert_eq!(library.add_root("other", RENDER), no_evictions());
        assert_eq!(library.module(&device, &key("main", &[])).expect("again"), id);
    }

    #[test]
    fn registering_the_same_source_again_keeps_the_cached_module() {
        let device = Device::null();
        let (mut library, id) = library_with_answer_module(&device);
        let same = "fn answer() -> u32 { return 1u; }";
        assert_eq!(library.add_module("answer", same).expect("module"), no_evictions());
        assert_eq!(library.module(&device, &key("main", &[])).expect("again"), id);
    }

    #[test]
    fn replacing_a_module_returns_the_evicted_ids_and_builds_a_new_module() {
        let device = Device::null();
        let (mut library, old) = library_with_answer_module(&device);
        let evicted = library
            .add_module("answer", "fn answer() -> u32 { return 2u; }")
            .expect("replaced");
        assert_eq!(evicted, [old]);
        assert_ne!(library.module(&device, &key("main", &[])).expect("new"), old);
    }

    #[test]
    fn replacing_a_root_returns_the_evicted_ids_and_builds_a_new_module() {
        let device = Device::null();
        let (mut library, old) = library_with_answer_module(&device);
        let evicted = library.add_root("main", "@compute @workgroup_size(1) fn main() {}");
        assert_eq!(evicted, [old]);
        assert_ne!(library.module(&device, &key("main", &[])).expect("new"), old);
    }

    #[test]
    fn a_module_creation_error_is_wrapped_and_the_same_key_succeeds_afterwards() {
        let device = FlakyDevice::new();
        device.fail_next_module.store(true, Ordering::SeqCst);
        let mut library = library();
        let error = library.module(&device, &key("render", &[])).expect_err("flaky");
        assert!(matches!(error, ShaderError::Rhi(_)), "{error:?}");
        library.module(&device, &key("render", &[])).expect("same key succeeds");
    }

    #[test]
    fn an_invalid_module_path_is_an_error() {
        assert!(matches!(
            ShaderLibrary::new().add_module("bad path", "fn f() {}"),
            Err(ShaderError::InvalidModulePath { .. })
        ));
    }
}

mod pipeline_cache {
    use super::*;

    fn compute_desc(
        module: pathon_rhi::ShaderModuleId,
        constants: Vec<(&'static str, pathon_rhi::OverrideValue)>,
    ) -> ComputePipelineDesc {
        ComputePipelineDesc {
            label: "permutation",
            bind_group_layouts: vec![],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants,
            },
            immediate_size: 0,
        }
    }

    fn render_desc(module: pathon_rhi::ShaderModuleId) -> RenderPipelineDesc {
        RenderPipelineDesc {
            label: "permutation",
            bind_group_layouts: vec![],
            vertex: ShaderStage {
                module,
                entry_point: "vs",
                constants: vec![],
            },
            vertex_buffers: vec![VertexBufferLayout {
                stride: 8,
                step_mode: VertexStepMode::Vertex,
                attributes: vec![VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 0,
                    location: 0,
                }],
            }],
            fragment: Some(ShaderStage {
                module,
                entry_point: "fs",
                constants: vec![],
            }),
            color_targets: vec![ColorTargetState::opaque(Format::Rgba8Unorm)],
            primitive: PrimitiveState::default(),
            depth: None,
            sample_count: 1,
            immediate_size: 0,
        }
    }

    #[test]
    fn an_equal_description_returns_the_cached_pipeline() {
        let device = Device::null();
        let mut cache = PipelineCache::new();
        let module = library().module(&device, &key("compute", &[])).expect("module");
        let desc = compute_desc(module, vec![]);
        assert_eq!(
            cache.compute(&device, &desc).expect("first"),
            cache.compute(&device, &desc).expect("second")
        );
    }

    #[test]
    fn constants_in_another_order_are_the_same_pipeline() {
        let device = Device::null();
        let mut cache = PipelineCache::new();
        let module = library().module(&device, &key("compute", &[])).expect("module");
        let ordered = compute_desc(module, vec![("a", 1.0.into()), ("b", 2.0.into())]);
        let swapped = compute_desc(module, vec![("b", 2.0.into()), ("a", 1.0.into())]);
        assert_eq!(
            cache.compute(&device, &ordered).expect("ordered"),
            cache.compute(&device, &swapped).expect("swapped")
        );
    }

    #[test]
    fn a_different_constant_value_or_module_is_another_pipeline() {
        let device = Device::null();
        let mut cache = PipelineCache::new();
        let mut library = library();
        let module = library.module(&device, &key("compute", &[])).expect("module");
        let wide = library.module(&device, &key("compute", &["wide"])).expect("wide");
        let base = compute_desc(module, vec![("a", 1.0.into())]);
        let other_value = compute_desc(module, vec![("a", 2.0.into())]);
        let other_module = compute_desc(wide, vec![("a", 1.0.into())]);
        let ids = [
            cache.compute(&device, &base).expect("base"),
            cache.compute(&device, &other_value).expect("value"),
            cache.compute(&device, &other_module).expect("module"),
        ];
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[..index].contains(id), "pipeline {index} repeats an earlier one");
        }
    }

    #[test]
    fn render_pipelines_differ_by_target_format_sample_count_vertex_layout_and_module() {
        let device = Device::null();
        let mut library = library();
        let module = library.module(&device, &key("render", &[])).expect("module");
        let other_module = library.module(&device, &key("render", &["variant"])).expect("other");
        let base = render_desc(module);

        let mut other_format = base.clone();
        other_format.color_targets = vec![ColorTargetState::opaque(Format::Bgra8Unorm)];
        let mut other_samples = base.clone();
        other_samples.sample_count = 4;
        let mut other_layout = base.clone();
        other_layout.vertex_buffers[0].stride = 16;
        let other_module_desc = render_desc(other_module);

        let mut cache = PipelineCache::new();
        let ids = [
            cache.render(&device, &base).expect("base"),
            cache.render(&device, &other_format).expect("format"),
            cache.render(&device, &other_samples).expect("samples"),
            cache.render(&device, &other_layout).expect("layout"),
            cache.render(&device, &other_module_desc).expect("module"),
        ];
        for (index, id) in ids.iter().enumerate() {
            assert!(!ids[..index].contains(id), "pipeline {index} repeats an earlier one");
        }
        assert_eq!(cache.render(&device, &base.clone()).expect("again"), ids[0]);
    }

    #[test]
    fn a_failed_creation_is_not_remembered_for_the_same_description() {
        let device = FlakyDevice::new();
        let module = library().module(&device, &key("compute", &[])).expect("module");
        let desc = compute_desc(module, vec![("a", 1.0.into())]);
        let mut cache = PipelineCache::new();
        device.fail_next_pipeline.store(true, Ordering::SeqCst);
        assert!(cache.compute(&device, &desc).is_err());
        cache.compute(&device, &desc).expect("same description succeeds");
    }

    #[test]
    fn duplicate_override_names_are_an_error_every_time() {
        let device = Device::null();
        let module = library().module(&device, &key("compute", &[])).expect("module");
        let duplicated = compute_desc(module, vec![("a", 1.0.into()), ("a", 2.0.into())]);
        let mut cache = PipelineCache::new();
        for _ in 0..2 {
            assert!(cache.compute(&device, &duplicated).is_err());
        }
    }

    #[test]
    fn render_constants_in_another_order_are_the_same_pipeline() {
        let device = Device::null();
        let module = library().module(&device, &key("render", &[])).expect("module");
        let mut ordered = render_desc(module);
        ordered.vertex.constants = vec![("a", 1.0.into()), ("b", 2.0.into())];
        ordered.fragment.as_mut().expect("fragment").constants = vec![("c", 1.0.into()), ("d", 2.0.into())];
        let mut swapped = ordered.clone();
        swapped.vertex.constants.reverse();
        swapped.fragment.as_mut().expect("fragment").constants.reverse();
        let mut cache = PipelineCache::new();
        assert_eq!(
            cache.render(&device, &ordered).expect("ordered"),
            cache.render(&device, &swapped).expect("swapped")
        );
    }

    #[test]
    fn the_label_is_part_of_the_pipeline_key() {
        let device = Device::null();
        let module = library().module(&device, &key("compute", &[])).expect("module");
        let first = compute_desc(module, vec![]);
        let mut relabelled = first.clone();
        relabelled.label = "other";
        let mut cache = PipelineCache::new();
        assert_ne!(
            cache.compute(&device, &first).expect("first"),
            cache.compute(&device, &relabelled).expect("relabelled")
        );
    }
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pathon_rhi::{
        BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindingResource, BindingType,
        BufferDesc, BufferUsage, CommandList, ComputePassDesc, DeviceApi, DeviceRequest, ShaderStages, StorageAccess,
    };
    use pathon_shader::reflect_bind_groups;

    use super::*;

    fn run(device: &Device, library: &mut ShaderLibrary, cache: &mut PipelineCache, names: &[&str]) -> u32 {
        let module = library.module(device, &key("compute", names)).expect("module");
        let layout = device
            .create_bind_group_layout(&BindGroupLayoutDesc {
                label: "permutation",
                entries: vec![BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::StorageBuffer {
                        access: StorageAccess::ReadWrite,
                    },
                    count: None,
                }],
            })
            .expect("layout");
        let output = device
            .create_buffer(&BufferDesc {
                label: "out",
                size: 4,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
            })
            .expect("output");
        let readback = device
            .create_buffer(&BufferDesc {
                label: "readback",
                size: 4,
                usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
            })
            .expect("readback");
        let group = device
            .create_bind_group(&BindGroupDesc {
                label: "permutation",
                layout,
                entries: vec![BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::whole_buffer(output),
                }],
            })
            .expect("group");
        let pipeline = cache
            .compute(
                device,
                &ComputePipelineDesc {
                    label: "permutation",
                    bind_group_layouts: vec![layout],
                    shader: ShaderStage {
                        module,
                        entry_point: "main",
                        constants: vec![],
                    },
                    immediate_size: 0,
                },
            )
            .expect("pipeline");
        let mut list = CommandList::new();
        {
            let mut pass = list.compute_pass(ComputePassDesc {
                label: "permutation",
                timestamps: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group);
            pass.dispatch(1, 1, 1);
        }
        list.copy_buffer_to_buffer(output, 0, readback, 0, 4);
        device.submit(list).expect("submit");
        let bytes = device.read_buffer(readback).expect("read");
        u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))
    }

    #[test]
    fn two_permutations_of_one_shader_compute_different_results() {
        let device = Device::build(&DeviceRequest {
            label: "permutations",
            ..Default::default()
        })
        .expect("a GPU adapter is required for gpu-tests");
        let mut library = library();
        let mut cache = PipelineCache::new();
        assert_eq!(run(&device, &mut library, &mut cache, &[]), 1);
        assert_eq!(run(&device, &mut library, &mut cache, &["wide"]), 4);
        assert_eq!(run(&device, &mut library, &mut cache, &["shadows"]), 10);
        assert_eq!(run(&device, &mut library, &mut cache, &["wide", "shadows"]), 40);
        let wgsl = ShaderComposer::new()
            .compose_with(COMPUTE, "c.wgsl", &features(&["wide"]))
            .expect("compose");
        assert_eq!(reflect_bind_groups(&wgsl, &["main"]).expect("reflect").len(), 1);
    }
}
