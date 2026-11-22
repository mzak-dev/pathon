use pathon_rhi_types::{CommandList, DeviceApi, MaybeSendSync, PassTimestamps};

// What a profiler needs from the frame (ADR-0026). The graph executor and the renderer call these hooks; every one is
// optional, and without an observer none runs.
pub trait FrameObserver: MaybeSendSync {
    fn begin_frame(&self, _device: &dyn DeviceApi, _frame: u64) {}

    // Around the recording of one graph pass (CPU side); always paired, also when recording fails.
    fn begin_pass(&self, _pass: &'static str) {}

    fn end_pass(&self, _pass: &'static str) {}

    // One GPU pass (`label`) recorded by the graph pass `pass` without timestamps of its own; the answer is written
    // into its description.
    fn gpu_pass(&self, _device: &dyn DeviceApi, _pass: &'static str, _label: &'static str) -> Option<PassTimestamps> {
        None
    }

    // The frame's commands are complete: the observer may append its own (resolving queries).
    fn end_frame(&self, _device: &dyn DeviceApi, _commands: &mut CommandList) {}

    // The frame's list reached the GPU, or in mode A was handed to the host.
    fn frame_committed(&self, _device: &dyn DeviceApi) {}

    // The frame's list was dropped: whatever `gpu_pass` handed out was never written.
    fn frame_abandoned(&self) {}

    // GPU objects of the old device are gone; drop references to them without destroying.
    fn device_reset(&self) {}
}
