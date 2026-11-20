// Shared helpers for the examples; see docs/examples.md.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use pathon::api::Camera;
use pathon::api::glam::Vec3;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    #[expect(clippy::print_stderr, reason = "the examples' logger writes to the console")]
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

// ADR-0007: libraries only log; the application installs the logger.
pub fn init_logging() {
    static LOGGER: StderrLogger = StderrLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

pub fn orbit_camera(time_seconds: f64, aspect_ratio: f32) -> Camera {
    #[expect(clippy::cast_possible_truncation, reason = "animation time only needs f32 precision")]
    let angle = time_seconds as f32 * 0.3;
    let position = Vec3::new(angle.sin() * 2.5, 1.2, angle.cos() * 2.5);
    Camera::look_at(position, Vec3::ZERO, std::f32::consts::FRAC_PI_3, aspect_ratio)
}

// Native wgpu returns already-completed futures, so the host examples need no executor.
pub fn block_on_ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("native wgpu futures complete immediately"),
    }
}
