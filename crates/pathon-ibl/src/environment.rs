use pathon_api::rhi::{Access, DeviceApi, Resource, SamplerId, TextureId, TextureViewId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IblTexture {
    pub texture: TextureId,
    pub view: TextureViewId,
}

/// A `Copy` handle over the persistent maps of one environment, outside the frame graph. The BRDF table and the
/// sampler belong to the [`IblBuilder`](crate::IblBuilder); [`Self::release`] frees only the cubes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IblEnvironment {
    /// Cosine-convolved environment, divided by pi (`ibl_diffuse`): one mip.
    pub irradiance: IblTexture,
    /// The environment filtered per roughness, mip `m` for roughness `m / (levels - 1)` (`ibl_specular`).
    pub prefiltered: IblTexture,
    /// Split-sum lookup `(scale, bias)` over `(n.v, roughness)` (`ibl_brdf`).
    pub brdf_lut: IblTexture,
    pub sampler: SamplerId,
    pub prefiltered_levels: u32,
    /// Counts the builds of one builder, so a consumer notices that its bind groups are stale.
    pub generation: u64,
}

impl IblEnvironment {
    /// The state the maps are left in, and the one a graph import starts and ends in.
    pub const STATE: Access = Access::SAMPLED;

    pub fn owned_resources(&self) -> [Resource; 4] {
        [
            Resource::TextureView(self.irradiance.view),
            Resource::Texture(self.irradiance.texture),
            Resource::TextureView(self.prefiltered.view),
            Resource::Texture(self.prefiltered.texture),
        ]
    }

    /// Destroys the maps now; while a frame in flight may read them, pass [`Self::owned_resources`] to
    /// `RecordCtx::release_later` instead.
    pub fn release(self, device: &dyn DeviceApi) {
        self.owned_resources()
            .into_iter()
            .for_each(|resource| device.destroy(resource));
    }
}
