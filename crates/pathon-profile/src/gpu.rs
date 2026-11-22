use pathon_api::rhi::{
    BufferDesc, BufferId, BufferUsage, CommandList, DeviceApi, Features, PassTimestamps, QuerySetDesc, QuerySetId,
    Resource, RhiError,
};
use pathon_diag::Diagnostic;

use crate::sink::GpuPassTiming;

const BYTES_PER_QUERY: u64 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PassName {
    pass: &'static str,
    label: &'static str,
}

#[derive(Debug)]
enum SlotState {
    Free,
    // The frame being recorded hands out query pairs.
    Recording(Vec<PassName>),
    // Submitted (or handed to the host) and waiting to be read back.
    Pending { frame: u64, passes: Vec<PassName> },
}

#[derive(Debug)]
struct Slot {
    query_set: QuerySetId,
    resolve: BufferId,
    readback: BufferId,
    state: SlotState,
}

#[derive(Debug)]
enum Readiness {
    Unchecked,
    Ready(Vec<Slot>),
    Unavailable,
}

// Timestamps of one frame live in one slot of a ring; a slot is read back `latency` frames after its frame, so the
// GPU is not waited on for the frame just recorded (ADR-0026).
#[derive(Debug)]
pub(crate) struct GpuTimer {
    capacity: u32,
    latency: u64,
    readiness: Readiness,
    current: Option<usize>,
    frame: u64,
    period_ns: f32,
    overflow_reported: bool,
}

impl GpuTimer {
    pub(crate) fn new(capacity: u32, latency: u64) -> Self {
        Self {
            capacity: capacity.max(1),
            latency,
            readiness: Readiness::Unchecked,
            current: None,
            frame: 0,
            period_ns: 1.0,
            overflow_reported: false,
        }
    }

    pub(crate) fn begin_frame(&mut self, device: &dyn DeviceApi, frame: u64) {
        self.frame = frame;
        if matches!(self.readiness, Readiness::Unchecked) {
            self.readiness = match self.create_slots(device) {
                Ok(slots) => Readiness::Ready(slots),
                Err(reason) => {
                    report_unavailable(&reason);
                    Readiness::Unavailable
                }
            };
        }
        self.period_ns = device.caps().timestamp_period_ns;
        let Readiness::Ready(slots) = &mut self.readiness else {
            return;
        };
        if let Some(unfinished) = self.current.take().and_then(|index| slots.get_mut(index)) {
            unfinished.state = SlotState::Free;
        }
        let index = free_or_oldest_slot(slots);
        slots[index].state = SlotState::Recording(Vec::new());
        self.current = Some(index);
    }

    pub(crate) fn gpu_pass(&mut self, pass: &'static str, label: &'static str) -> Option<PassTimestamps> {
        let Readiness::Ready(slots) = &mut self.readiness else {
            return None;
        };
        let slot = slots.get_mut(self.current?)?;
        let SlotState::Recording(passes) = &mut slot.state else {
            return None;
        };
        let index = u32::try_from(passes.len()).ok()?;
        if index >= self.capacity {
            if !std::mem::replace(&mut self.overflow_reported, true) {
                pathon_diag::emit(&overflow_diagnostic(self.capacity));
            }
            return None;
        }
        passes.push(PassName { pass, label });
        Some(PassTimestamps {
            query_set: slot.query_set,
            begin_index: index * 2,
            end_index: index * 2 + 1,
        })
    }

    pub(crate) fn end_frame(&mut self, commands: &mut CommandList) {
        let Readiness::Ready(slots) = &self.readiness else {
            return;
        };
        let Some(slot) = self.current.and_then(|index| slots.get(index)) else {
            return;
        };
        let SlotState::Recording(passes) = &slot.state else {
            return;
        };
        let Ok(queries) = u32::try_from(passes.len() * 2) else {
            return;
        };
        if queries == 0 {
            return;
        }
        commands.resolve_queries(slot.query_set, 0..queries, slot.resolve, 0);
        commands.copy_buffer_to_buffer(slot.resolve, 0, slot.readback, 0, u64::from(queries) * BYTES_PER_QUERY);
    }

    // The slot's frame reached the GPU: it is read back once `latency` newer frames have been recorded.
    pub(crate) fn frame_committed(&mut self) {
        let frame = self.frame;
        let Readiness::Ready(slots) = &mut self.readiness else {
            return;
        };
        let Some(slot) = self.current.take().and_then(|index| slots.get_mut(index)) else {
            return;
        };
        slot.state = match std::mem::replace(&mut slot.state, SlotState::Free) {
            SlotState::Recording(passes) if !passes.is_empty() => SlotState::Pending { frame, passes },
            _ => SlotState::Free,
        };
    }

    pub(crate) fn frame_abandoned(&mut self) {
        let Readiness::Ready(slots) = &mut self.readiness else {
            return;
        };
        if let Some(slot) = self.current.take().and_then(|index| slots.get_mut(index)) {
            slot.state = SlotState::Free;
        }
    }

    // Slots whose frame is at least `latency` frames older than `newest` (all of them with `None`), oldest first.
    pub(crate) fn read_ready(
        &mut self,
        device: &dyn DeviceApi,
        newest: Option<u64>,
    ) -> Vec<(u64, f32, Vec<GpuPassTiming>)> {
        let Readiness::Ready(slots) = &mut self.readiness else {
            return Vec::new();
        };
        let mut due: Vec<usize> = (0..slots.len())
            .filter(|&index| match (&slots[index].state, newest) {
                (SlotState::Pending { frame, .. }, Some(newest)) => frame + self.latency <= newest,
                (SlotState::Pending { .. }, None) => true,
                _ => false,
            })
            .collect();
        due.sort_by_key(|&index| match &slots[index].state {
            SlotState::Pending { frame, .. } => *frame,
            _ => u64::MAX,
        });
        let mut results = Vec::new();
        for index in due {
            let slot = &mut slots[index];
            let SlotState::Pending { frame, passes } = std::mem::replace(&mut slot.state, SlotState::Free) else {
                continue;
            };
            if let Ok(bytes) = device.read_buffer(slot.readback) {
                results.push((frame, self.period_ns, timings_from(&bytes, &passes)));
            }
        }
        results
    }

    pub(crate) fn release(&mut self, device: &dyn DeviceApi) {
        if let Readiness::Ready(slots) = std::mem::replace(&mut self.readiness, Readiness::Unchecked) {
            for slot in slots {
                device.destroy(Resource::QuerySet(slot.query_set));
                device.destroy(Resource::Buffer(slot.resolve));
                device.destroy(Resource::Buffer(slot.readback));
            }
        }
        self.current = None;
    }

    // Objects of a device that is gone cannot be destroyed; they are forgotten and rebuilt on the next frame.
    pub(crate) fn forget_device(&mut self) {
        self.readiness = Readiness::Unchecked;
        self.current = None;
    }

    fn create_slots(&self, device: &dyn DeviceApi) -> Result<Vec<Slot>, String> {
        if cfg!(target_arch = "wasm32") {
            return Err("the web has no blocking buffer readback".into());
        }
        if !device.caps().has(Features::TIMESTAMP_QUERY) {
            return Err("the device lacks Features::TIMESTAMP_QUERY".into());
        }
        let ring = usize::try_from(self.latency + 1).map_err(|error| error.to_string())?;
        (0..ring)
            .map(|_| self.create_slot(device).map_err(|error| error.to_string()))
            .collect()
    }

    fn create_slot(&self, device: &dyn DeviceApi) -> Result<Slot, RhiError> {
        let queries = self.capacity * 2;
        let size = u64::from(queries) * BYTES_PER_QUERY;
        Ok(Slot {
            query_set: device.create_query_set(&QuerySetDesc {
                label: "pathon profile timestamps",
                count: queries,
            })?,
            resolve: device.create_buffer(&BufferDesc {
                label: "pathon profile resolve",
                size,
                usage: BufferUsage::QUERY_RESOLVE | BufferUsage::COPY_SRC,
            })?,
            readback: device.create_buffer(&BufferDesc {
                label: "pathon profile readback",
                size,
                usage: BufferUsage::COPY_DST | BufferUsage::MAP_READ,
            })?,
            state: SlotState::Free,
        })
    }
}

// The ring is as long as the latency plus one, so a free slot exists unless the host never reads its frames back.
fn free_or_oldest_slot(slots: &[Slot]) -> usize {
    let oldest_pending = |index: &usize| match &slots[*index].state {
        SlotState::Pending { frame, .. } => *frame,
        _ => 0,
    };
    (0..slots.len())
        .find(|&index| matches!(slots[index].state, SlotState::Free))
        .or_else(|| (0..slots.len()).min_by_key(oldest_pending))
        .unwrap_or(0)
}

fn timings_from(bytes: &[u8], passes: &[PassName]) -> Vec<GpuPassTiming> {
    let (words, _) = bytes.as_chunks::<{ size_of::<u64>() }>();
    let ticks: Vec<u64> = words.iter().map(|word| u64::from_le_bytes(*word)).collect();
    let (pairs, _) = ticks.as_chunks::<2>();
    passes
        .iter()
        .zip(pairs)
        .map(|(name, [begin, end])| GpuPassTiming::new(name.pass, name.label, *begin, *end))
        .collect()
}

fn report_unavailable(reason: &str) {
    pathon_diag::emit(
        &Diagnostic::warning("T0001", format!("GPU pass timings are off: {reason}"))
            .with_hint("request Features::TIMESTAMP_QUERY in DeviceRequest::optional_features; CPU spans keep working"),
    );
}

fn overflow_diagnostic(capacity: u32) -> Diagnostic {
    Diagnostic::warning(
        "T0002",
        format!("a frame has more than {capacity} GPU passes; the rest are not timed"),
    )
    .with_hint("raise the capacity with ProfilerBuilder::with_gpu_pass_capacity")
}
