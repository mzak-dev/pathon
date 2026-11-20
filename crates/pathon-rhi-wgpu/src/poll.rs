use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

// Native wgpu resolves adapter, device and error-scope futures before returning them, so a single poll completes
// them without an executor. On the web they are real promises and must be awaited instead.
pub(crate) fn ready<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}
