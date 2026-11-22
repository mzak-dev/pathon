// M0-05: a real window on the real platform. The test resizes it from code and waits for `PlatformEvent::Resized`, the
// same event a user dragging the window edge produces. A scale change cannot be forced from code, so on X11 the CI
// job starts the test with `WINIT_X11_SCALE_FACTOR` and `PATHON_EXPECT_SCALE` and the test checks that a non-1 scale
// reaches `scale_factor()` and the logical-to-physical size. Needs a display: `PATHON_REQUIRE_WINDOW=1` turns a
// missing one into a failure instead of a skip. Harness off: macOS only allows the event loop on the main thread.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a test without the harness reports on the console"
)]

use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pathon::WinitPlatform;
use pathon::platform::{App, PhysicalSize, Platform, PlatformCtx, PlatformEvent, PlatformWindow, WindowDesc};

const LOGICAL_WIDTH: f64 = 400.0;
const LOGICAL_HEIGHT: f64 = 300.0;
const GROWTH: PhysicalSize = PhysicalSize {
    width: 160,
    height: 100,
};
const SETTLE_FRAMES: u32 = 5;
const RESIZE_TIMEOUT: Duration = Duration::from_secs(10);
const SCALE_TOLERANCE: f64 = 0.01;

#[derive(Default, Debug)]
struct Report {
    initial_size: Option<PhysicalSize>,
    scale_factor: Option<f64>,
    resized: Vec<PhysicalSize>,
    scale_changes: Vec<f64>,
    outcome: Option<Result<(), String>>,
}

type Shared = Arc<Mutex<Report>>;

fn lock(report: &Shared) -> MutexGuard<'_, Report> {
    report.lock().expect("the probe never panics while holding the report")
}

struct Probe {
    report: Shared,
    window: Option<Arc<dyn PlatformWindow>>,
    frames: u32,
    resize_requested_at: Option<Instant>,
}

impl Probe {
    fn finish(&self, ctx: &mut dyn PlatformCtx, outcome: Result<(), String>) {
        lock(&self.report).outcome = Some(outcome);
        ctx.exit();
    }

    fn open_window(&mut self, ctx: &mut dyn PlatformCtx) {
        let desc = WindowDesc::new("pathon — window events", LOGICAL_WIDTH, LOGICAL_HEIGHT);
        match ctx.create_window(&desc) {
            Ok(window) => {
                let mut report = lock(&self.report);
                report.initial_size = Some(window.inner_size());
                report.scale_factor = Some(window.scale_factor());
                drop(report);
                self.window = Some(window);
            }
            Err(error) => self.finish(ctx, Err(format!("window creation failed: {error}"))),
        }
    }

    fn request_growth(&mut self) {
        let (Some(window), Some(initial)) = (&self.window, lock(&self.report).initial_size) else {
            return;
        };
        window.request_inner_size(PhysicalSize {
            width: initial.width + GROWTH.width,
            height: initial.height + GROWTH.height,
        });
        self.resize_requested_at = Some(Instant::now());
    }

    fn check_resized(&self, ctx: &mut dyn PlatformCtx, size: PhysicalSize) {
        let Some(window) = &self.window else {
            return;
        };
        let initial = lock(&self.report).initial_size;
        if self.resize_requested_at.is_none() || Some(size) == initial {
            return;
        }
        let outcome = if window.inner_size() == size {
            Ok(())
        } else {
            Err(format!(
                "Resized said {size:?} but inner_size() is {:?}",
                window.inner_size()
            ))
        };
        self.finish(ctx, outcome);
    }
}

impl App for Probe {
    fn on_event(&mut self, ctx: &mut dyn PlatformCtx, event: PlatformEvent) {
        match event {
            PlatformEvent::SurfacesAvailable if self.window.is_none() => self.open_window(ctx),
            PlatformEvent::Resized { size, .. } => {
                lock(&self.report).resized.push(size);
                self.check_resized(ctx, size);
            }
            PlatformEvent::ScaleChanged { scale_factor, .. } => lock(&self.report).scale_changes.push(scale_factor),
            PlatformEvent::CloseRequested(_) => self.finish(ctx, Err("the window was closed".into())),
            _ => {}
        }
    }

    fn on_frame(&mut self, ctx: &mut dyn PlatformCtx) {
        if self.window.is_none() {
            return;
        }
        self.frames += 1;
        match self.resize_requested_at {
            None if self.frames >= SETTLE_FRAMES => self.request_growth(),
            Some(requested_at) if requested_at.elapsed() > RESIZE_TIMEOUT => self.finish(
                ctx,
                Err(format!(
                    "no Resized event within {RESIZE_TIMEOUT:?} of the resize request"
                )),
            ),
            _ => {}
        }
    }
}

fn has_display() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn check_expected_scale(report: &Report) -> Result<(), String> {
    let Some(expected) = std::env::var("PATHON_EXPECT_SCALE")
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
    else {
        return Ok(());
    };
    let scale = report.scale_factor.ok_or("no scale factor recorded")?;
    if (scale - expected).abs() > SCALE_TOLERANCE {
        return Err(format!("scale_factor() is {scale}, expected {expected}"));
    }
    let initial = report.initial_size.ok_or("no initial size recorded")?;
    let expected_width = (LOGICAL_WIDTH * expected).round();
    if (f64::from(initial.width) - expected_width).abs() > 1.0 {
        return Err(format!(
            "initial width is {} physical pixels, expected about {expected_width} for scale {expected}",
            initial.width
        ));
    }
    Ok(())
}

fn main() -> ExitCode {
    if !has_display() {
        if std::env::var_os("PATHON_REQUIRE_WINDOW").is_some() {
            eprintln!("window_events: no display (DISPLAY / WAYLAND_DISPLAY) and PATHON_REQUIRE_WINDOW is set");
            return ExitCode::FAILURE;
        }
        eprintln!("window_events: skipped, no display");
        return ExitCode::SUCCESS;
    }
    let report: Shared = Arc::default();
    let probe = Probe {
        report: Arc::clone(&report),
        window: None,
        frames: 0,
        resize_requested_at: None,
    };
    if let Err(error) = WinitPlatform::new().run(Box::new(probe)) {
        eprintln!("window_events: the platform failed: {error}");
        return ExitCode::FAILURE;
    }
    let report = lock(&report);
    println!("window_events: {report:?}");
    let outcome = match &report.outcome {
        Some(Ok(())) => check_expected_scale(&report),
        Some(Err(message)) => Err(message.clone()),
        None => Err("the event loop ended without an outcome".into()),
    };
    match outcome {
        Ok(()) => {
            println!("window_events: ok");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("window_events: FAILED: {message}");
            ExitCode::FAILURE
        }
    }
}
