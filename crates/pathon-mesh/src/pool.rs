use std::collections::VecDeque;
use std::ops::Range;

use pathon_api::rhi::{
    BufferDesc, BufferId, BufferUsage, DeviceApi, Id, IndexFormat, Pool, RawHandle, RenderPassEncoder, Resource,
};

use crate::chunked_allocator::{Allocation, AllocationError, AllocatorStats, ChunkPolicy, ChunkedAllocator};
use crate::data::{Bounds, MeshData, ValidatedMesh, validate};
use crate::error::{ArenaKind, MeshError};
use crate::layout::VertexLayout;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MeshId(RawHandle);

impl Id for MeshId {
    const KIND: &'static str = "mesh";

    fn from_raw(raw: RawHandle) -> Self {
        Self(raw)
    }

    fn raw(self) -> RawHandle {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct PoolDesc {
    pub chunk_vertices: u32,
    pub chunk_indices: u32,
    pub max_vertices: u64,
    pub max_indices: u64,
    pub frames_in_flight: u64,
}

impl Default for PoolDesc {
    fn default() -> Self {
        Self {
            chunk_vertices: 1 << 18,
            chunk_indices: 1 << 20,
            max_vertices: 1 << 23,
            max_indices: 1 << 25,
            frames_in_flight: 2,
        }
    }
}

impl PoolDesc {
    #[must_use]
    pub fn with_chunk_sizes(self, chunk_vertices: u32, chunk_indices: u32) -> Self {
        Self {
            chunk_vertices,
            chunk_indices,
            ..self
        }
    }

    #[must_use]
    pub fn with_budget(self, max_vertices: u64, max_indices: u64) -> Self {
        Self {
            max_vertices,
            max_indices,
            ..self
        }
    }

    #[must_use]
    pub fn with_frames_in_flight(self, frames_in_flight: u64) -> Self {
        Self {
            frames_in_flight,
            ..self
        }
    }
}

// Where a mesh lives. Vertex and index chunks are independent; `base_vertex` goes to the draw call (or the indirect
// arguments), and indices in the pool are relative to the mesh.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct MeshLocation {
    pub vertex_chunk: u32,
    pub base_vertex: u32,
    pub vertex_count: u32,
    pub index_chunk: u32,
    pub first_index: u32,
    pub index_count: u32,
}

impl MeshLocation {
    pub fn indices(&self) -> Range<u32> {
        self.first_index..self.first_index + self.index_count
    }

    pub fn base_vertex_signed(&self) -> i32 {
        i32::try_from(self.base_vertex).expect("a vertex chunk never exceeds i32::MAX vertices")
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct MeshInfo {
    pub label: &'static str,
    pub location: MeshLocation,
    pub bounds: Bounds,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct ArenaStats {
    pub chunks: usize,
    pub capacity: u64,
    pub used: u64,
    // Removed but not yet reusable: the GPU may still read them.
    pub pending: u64,
    pub free: u64,
    pub largest_free: u64,
}

impl ArenaStats {
    // 0 when all free space is one range, towards 1 as it splits into pieces.
    pub fn fragmentation(&self) -> f32 {
        if self.free == 0 {
            return 0.0;
        }
        1.0 - self.largest_free as f32 / self.free as f32
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct PoolStats {
    pub meshes: usize,
    pub vertices: ArenaStats,
    pub indices: ArenaStats,
}

#[derive(Debug)]
struct PendingRemoval {
    frame: u64,
    location: MeshLocation,
}

// Many meshes of one vertex layout in a few large buffers, so a draw switches buffers rarely and GPU-driven paths
// (indirect, vertex pull, ray tracing) see all geometry through the same few bindings. See ADR-0027.
#[derive(Debug)]
pub struct GeometryPool {
    label: &'static str,
    layout: VertexLayout,
    frames_in_flight: u64,
    vertices: ChunkedAllocator,
    indices: ChunkedAllocator,
    vertex_buffers: Vec<Option<Vec<BufferId>>>,
    index_buffers: Vec<Option<BufferId>>,
    meshes: Pool<MeshId, MeshInfo>,
    pending: VecDeque<PendingRemoval>,
    pending_vertices: u64,
    pending_indices: u64,
}

fn chunk_policy(chunk: u64, budget: u64, device_limit: u64, format_limit: u64) -> ChunkPolicy {
    let max_chunk_capacity = device_limit.min(format_limit).min(budget).max(1);
    ChunkPolicy {
        chunk_capacity: chunk.clamp(1, max_chunk_capacity),
        max_chunk_capacity,
        max_total: budget.max(1),
    }
}

fn unit(value: u64) -> u32 {
    u32::try_from(value).expect("chunk limits keep unit offsets within u32")
}

impl GeometryPool {
    pub fn new(device: &dyn DeviceApi, label: &'static str, layout: VertexLayout, desc: PoolDesc) -> Self {
        let limits = &device.caps().limits;
        // A chunk can be bound whole as a storage buffer (vertex pull), so it must fit that binding too.
        let binding_limit = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size);
        let vertex_policy = chunk_policy(
            u64::from(desc.chunk_vertices),
            desc.max_vertices,
            binding_limit / layout.max_stride(),
            u64::from(i32::MAX.cast_unsigned()),
        );
        let index_policy = chunk_policy(
            u64::from(desc.chunk_indices),
            desc.max_indices,
            binding_limit / 4,
            u64::from(u32::MAX),
        );
        Self {
            label,
            layout,
            frames_in_flight: desc.frames_in_flight,
            vertices: ChunkedAllocator::new(vertex_policy),
            indices: ChunkedAllocator::new(index_policy),
            vertex_buffers: Vec::new(),
            index_buffers: Vec::new(),
            meshes: Pool::new(),
            pending: VecDeque::new(),
            pending_vertices: 0,
            pending_indices: 0,
        }
    }

    pub fn layout(&self) -> &VertexLayout {
        &self.layout
    }

    pub fn mesh(&self, mesh: MeshId) -> Option<&MeshInfo> {
        self.meshes.get(mesh)
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    pub fn vertex_buffers(&self, chunk: u32) -> Option<&[BufferId]> {
        self.vertex_buffers.get(chunk as usize)?.as_deref()
    }

    pub fn index_buffer(&self, chunk: u32) -> Option<BufferId> {
        *self.index_buffers.get(chunk as usize)?
    }

    pub fn upload(&mut self, device: &dyn DeviceApi, data: &MeshData<'_>) -> Result<MeshId, MeshError> {
        let mesh = validate(&self.layout, data)?;
        let vertices = self.allocate_vertices(device, data.label, mesh.vertex_count)?;
        let indices = match self.allocate_indices(device, data.label, mesh.indices.len() as u64) {
            Ok(indices) => indices,
            Err(error) => {
                self.free_vertices(device, vertices.chunk, vertices.range);
                return Err(error);
            }
        };
        if let Err(error) = self.write_mesh(device, &vertices, &indices, data, &mesh) {
            self.free_vertices(device, vertices.chunk, vertices.range);
            self.free_indices(device, indices.chunk, indices.range);
            return Err(error);
        }
        let location = MeshLocation {
            vertex_chunk: vertices.chunk,
            base_vertex: unit(vertices.range.start),
            vertex_count: unit(mesh.vertex_count),
            index_chunk: indices.chunk,
            first_index: unit(indices.range.start),
            index_count: unit(indices.range.end - indices.range.start),
        };
        Ok(self.meshes.insert(MeshInfo {
            label: data.label,
            location,
            bounds: mesh.bounds,
        }))
    }

    fn allocation_error(
        &self,
        kind: ArenaKind,
        label: &'static str,
        requested: u64,
        error: AllocationError,
    ) -> MeshError {
        let pending = match kind {
            ArenaKind::Indices => self.pending_indices,
            _ => self.pending_vertices,
        };
        match error {
            AllocationError::Empty => MeshError::InvalidData {
                label,
                reason: "a mesh needs at least one vertex and one index".to_owned(),
            },
            AllocationError::TooLarge { max } => MeshError::TooLarge {
                label,
                kind,
                requested,
                max,
            },
            AllocationError::Exhausted { free } => MeshError::Exhausted {
                pool: self.label,
                kind,
                requested,
                free,
                pending,
            },
            AllocationError::Fragmented { free, largest_free } => MeshError::Fragmented {
                pool: self.label,
                kind,
                requested,
                free,
                largest_free,
            },
        }
    }

    fn allocate_vertices(
        &mut self,
        device: &dyn DeviceApi,
        label: &'static str,
        count: u64,
    ) -> Result<Allocation, MeshError> {
        let allocation = self
            .vertices
            .allocate(count)
            .map_err(|error| self.allocation_error(ArenaKind::Vertices, label, count, error))?;
        if let Some(capacity) = allocation.opened_capacity
            && let Err(error) = self.create_vertex_chunk(device, allocation.chunk, capacity)
        {
            self.vertices.abandon_opened(allocation.chunk);
            return Err(error);
        }
        Ok(allocation)
    }

    fn allocate_indices(
        &mut self,
        device: &dyn DeviceApi,
        label: &'static str,
        count: u64,
    ) -> Result<Allocation, MeshError> {
        let allocation = self
            .indices
            .allocate(count)
            .map_err(|error| self.allocation_error(ArenaKind::Indices, label, count, error))?;
        if let Some(capacity) = allocation.opened_capacity
            && let Err(error) = self.create_index_chunk(device, allocation.chunk, capacity)
        {
            self.indices.abandon_opened(allocation.chunk);
            return Err(error);
        }
        Ok(allocation)
    }

    fn create_vertex_chunk(&mut self, device: &dyn DeviceApi, chunk: u32, capacity: u64) -> Result<(), MeshError> {
        let mut buffers = Vec::with_capacity(self.layout.streams().len());
        for stream in self.layout.streams() {
            let created = device.create_buffer(&BufferDesc {
                label: self.label,
                size: capacity * stream.stride,
                usage: BufferUsage::VERTEX | BufferUsage::STORAGE | BufferUsage::COPY_DST | BufferUsage::COPY_SRC,
            });
            match created {
                Ok(buffer) => buffers.push(buffer),
                Err(error) => {
                    destroy_all(device, &buffers);
                    return Err(error.into());
                }
            }
        }
        store_chunk(&mut self.vertex_buffers, chunk, Some(buffers));
        Ok(())
    }

    fn create_index_chunk(&mut self, device: &dyn DeviceApi, chunk: u32, capacity: u64) -> Result<(), MeshError> {
        let buffer = device.create_buffer(&BufferDesc {
            label: self.label,
            size: capacity * 4,
            usage: BufferUsage::INDEX | BufferUsage::STORAGE | BufferUsage::COPY_DST | BufferUsage::COPY_SRC,
        })?;
        store_chunk(&mut self.index_buffers, chunk, Some(buffer));
        Ok(())
    }

    fn write_mesh(
        &self,
        device: &dyn DeviceApi,
        vertices: &Allocation,
        indices: &Allocation,
        data: &MeshData<'_>,
        mesh: &ValidatedMesh<'_>,
    ) -> Result<(), MeshError> {
        let buffers = self.vertex_buffers(vertices.chunk).ok_or(MeshError::UnknownMesh)?;
        for ((buffer, stream), bytes) in buffers.iter().zip(self.layout.streams()).zip(&data.streams) {
            device.write_buffer(*buffer, vertices.range.start * stream.stride, bytes)?;
        }
        let index_buffer = self.index_buffer(indices.chunk).ok_or(MeshError::UnknownMesh)?;
        device.write_buffer(
            index_buffer,
            indices.range.start * 4,
            bytemuck::cast_slice(&mesh.indices),
        )?;
        Ok(())
    }

    fn free_vertices(&mut self, device: &dyn DeviceApi, chunk: u32, range: Range<u64>) {
        if let Some(released) = self.vertices.free(chunk, range)
            && let Some(buffers) = self.vertex_buffers.get_mut(released as usize).and_then(Option::take)
        {
            destroy_all(device, &buffers);
        }
    }

    fn free_indices(&mut self, device: &dyn DeviceApi, chunk: u32, range: Range<u64>) {
        if let Some(released) = self.indices.free(chunk, range)
            && let Some(buffer) = self.index_buffers.get_mut(released as usize).and_then(Option::take)
        {
            device.destroy(Resource::Buffer(buffer));
        }
    }

    // The handle stops working now; the ranges stay reserved until the GPU is done with `frame` (ADR-0022).
    pub fn remove(&mut self, mesh: MeshId, frame: u64) -> Result<(), MeshError> {
        let info = self.meshes.remove(mesh).ok_or(MeshError::UnknownMesh)?;
        self.pending_vertices += u64::from(info.location.vertex_count);
        self.pending_indices += u64::from(info.location.index_count);
        self.pending.push_back(PendingRemoval {
            frame,
            location: info.location,
        });
        Ok(())
    }

    // Returns how many removed meshes gave their ranges back.
    pub fn begin_frame(&mut self, device: &dyn DeviceApi, frame: u64) -> usize {
        let mut reclaimed = 0;
        while let Some(removal) = self.pending.front() {
            if frame < removal.frame + self.frames_in_flight {
                break;
            }
            let location = removal.location;
            self.pending.pop_front();
            let vertices = u64::from(location.base_vertex)..u64::from(location.base_vertex + location.vertex_count);
            let indices = u64::from(location.first_index)..u64::from(location.first_index + location.index_count);
            self.pending_vertices -= vertices.end - vertices.start;
            self.pending_indices -= indices.end - indices.start;
            self.free_vertices(device, location.vertex_chunk, vertices);
            self.free_indices(device, location.index_chunk, indices);
            reclaimed += 1;
        }
        reclaimed
    }

    pub fn bind(&self, pass: &mut RenderPassEncoder<'_>, mesh: MeshId) -> Option<MeshLocation> {
        let location = self.meshes.get(mesh)?.location;
        let buffers = self.vertex_buffers(location.vertex_chunk)?;
        let index_buffer = self.index_buffer(location.index_chunk)?;
        for (slot, buffer) in (0..).zip(buffers) {
            pass.set_vertex_buffer(slot, *buffer, 0);
        }
        pass.set_index_buffer(index_buffer, IndexFormat::Uint32, 0);
        Some(location)
    }

    // Binds the mesh's chunk buffers and draws it. Callers that sort draws by chunk bind once per chunk instead.
    pub fn draw(&self, pass: &mut RenderPassEncoder<'_>, mesh: MeshId, instances: Range<u32>) -> Result<(), MeshError> {
        let location = self.bind(pass, mesh).ok_or(MeshError::UnknownMesh)?;
        pass.draw_indexed(location.indices(), location.base_vertex_signed(), instances);
        Ok(())
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            meshes: self.meshes.len(),
            vertices: arena_stats(self.vertices.stats(), self.pending_vertices),
            indices: arena_stats(self.indices.stats(), self.pending_indices),
        }
    }

    // After device loss every buffer is dead: all handles become invalid and the application uploads again.
    pub fn forget_gpu(&mut self) {
        self.meshes.clear();
        self.pending.clear();
        self.pending_vertices = 0;
        self.pending_indices = 0;
        self.vertex_buffers.clear();
        self.index_buffers.clear();
        self.vertices = ChunkedAllocator::new(self.vertices.policy());
        self.indices = ChunkedAllocator::new(self.indices.policy());
    }

    // Destroys every buffer at once, so the caller must be sure the GPU no longer reads the pool.
    pub fn release_all(&mut self, device: &dyn DeviceApi) {
        for buffer in self.vertex_buffers.iter().flatten().flatten() {
            device.destroy(Resource::Buffer(*buffer));
        }
        for buffer in self.index_buffers.iter().flatten() {
            device.destroy(Resource::Buffer(*buffer));
        }
        self.forget_gpu();
    }
}

fn arena_stats(stats: AllocatorStats, pending: u64) -> ArenaStats {
    ArenaStats {
        chunks: stats.chunks,
        capacity: stats.capacity,
        used: stats.allocated - pending,
        pending,
        free: stats.free,
        largest_free: stats.largest_free,
    }
}

fn destroy_all(device: &dyn DeviceApi, buffers: &[BufferId]) {
    for buffer in buffers {
        device.destroy(Resource::Buffer(*buffer));
    }
}

fn store_chunk<T>(chunks: &mut Vec<Option<T>>, chunk: u32, value: Option<T>) {
    let index = chunk as usize;
    if chunks.len() <= index {
        chunks.resize_with(index + 1, || None);
    }
    chunks[index] = value;
}
