use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use pathon_platform::{PhysicalSize, PlatformEvent, WindowId};

/// Events the platform produces itself, outside of winit's loop, to be delivered on the next loop iteration.
#[derive(Clone, Debug, Default)]
pub(crate) struct PendingEvents(Arc<Mutex<VecDeque<PlatformEvent>>>);

impl PendingEvents {
    fn queue(&self) -> MutexGuard<'_, VecDeque<PlatformEvent>> {
        self.0.lock().expect("nothing panics while holding the queue")
    }

    pub(crate) fn push(&self, event: PlatformEvent) {
        self.queue().push_back(event);
    }

    pub(crate) fn drain(&self) -> Vec<PlatformEvent> {
        self.queue().drain(..).collect()
    }

    // Wayland (and mobile) apply a size request at once and winit reports it only as the return value, never as a
    // `Resized` event; the contract says the app always gets one.
    pub(crate) fn push_resize_applied_at_once(
        &self,
        window: WindowId,
        before: PhysicalSize,
        applied_at_once: Option<PhysicalSize>,
    ) {
        if let Some(size) = resized_at_once(before, applied_at_once) {
            self.push(PlatformEvent::Resized { window, size });
        }
    }
}

// `None` means winit will report the new size as an event itself; an unchanged size is not a resize (iOS and Android
// answer every request with the current size).
fn resized_at_once(before: PhysicalSize, applied_at_once: Option<PhysicalSize>) -> Option<PhysicalSize> {
    applied_at_once.filter(|applied| *applied != before)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: WindowId = WindowId(7);
    const BEFORE: PhysicalSize = PhysicalSize {
        width: 400,
        height: 300,
    };
    const AFTER: PhysicalSize = PhysicalSize {
        width: 560,
        height: 400,
    };

    fn resized(size: PhysicalSize) -> PlatformEvent {
        PlatformEvent::Resized { window: WINDOW, size }
    }

    #[test]
    fn size_applied_at_once_becomes_a_resized_event() {
        let pending = PendingEvents::default();
        pending.push_resize_applied_at_once(WINDOW, BEFORE, Some(AFTER));
        assert_eq!(pending.drain(), vec![resized(AFTER)]);
    }

    #[test]
    fn size_winit_reports_later_adds_no_event() {
        let pending = PendingEvents::default();
        pending.push_resize_applied_at_once(WINDOW, BEFORE, None);
        assert_eq!(pending.drain(), Vec::new());
    }

    #[test]
    fn unchanged_size_adds_no_event() {
        let pending = PendingEvents::default();
        pending.push_resize_applied_at_once(WINDOW, BEFORE, Some(BEFORE));
        assert_eq!(pending.drain(), Vec::new());
    }

    #[test]
    fn drain_empties_the_queue_in_order() {
        let pending = PendingEvents::default();
        pending.push_resize_applied_at_once(WINDOW, BEFORE, Some(AFTER));
        pending.push_resize_applied_at_once(WINDOW, AFTER, Some(BEFORE));
        assert_eq!(pending.drain(), vec![resized(AFTER), resized(BEFORE)]);
        assert_eq!(pending.drain(), Vec::new());
    }

    #[test]
    fn clones_share_one_queue() {
        let pending = PendingEvents::default();
        let window_side = pending.clone();
        window_side.push_resize_applied_at_once(WINDOW, BEFORE, Some(AFTER));
        assert_eq!(pending.drain(), vec![resized(AFTER)]);
    }
}
