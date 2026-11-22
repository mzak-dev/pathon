// ADR-0014: the `clear` example in the browser, through the thread-free runner.
#![cfg(target_arch = "wasm32")]

use pathon::platform::WindowDesc;
use pathon::rhi::{Color, DeviceApi, DeviceRequest, Features};
use pathon::{Renderer, ViewInput, WinitPlatform};
use wasm_bindgen::prelude::wasm_bindgen;

const LOG_EVERY_FRAMES: u64 = 120;

struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("[{}] {}: {}", record.level(), record.target(), record.args()).into();
        match record.level() {
            log::Level::Error => web_sys::console::error_1(&line),
            log::Level::Warn => web_sys::console::warn_1(&line),
            _ => web_sys::console::log_1(&line),
        }
    }

    fn flush(&self) {}
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    if log::set_logger(&ConsoleLogger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    // Every optional feature, so the logged Caps can be compared with what the browser's adapter offers.
    let builder = Renderer::builder()
        .with_clear_color(Color::rgba(0.1, 0.2, 0.35, 1.0))
        .with_device_request(DeviceRequest {
            optional_features: Features::all(),
            ..DeviceRequest::default()
        });
    let windows = vec![WindowDesc::new("pathon — clear", 960.0, 540.0)];
    let mut frames = 0_u64;
    let result = pathon::runtime::run_with_hook(
        WinitPlatform::new(),
        builder,
        windows,
        |frame| {
            frame
                .windows
                .iter()
                .map(|(target, _)| ViewInput {
                    target: *target,
                    camera: pathon::api::Camera::default(),
                })
                .collect()
        },
        move |renderer, _| {
            if frames == 0 {
                log::info!("caps: {:?}", renderer.device().caps());
            }
            if frames.is_multiple_of(LOG_EVERY_FRAMES) {
                log::info!("frame {frames}");
            }
            frames += 1;
        },
    );
    if let Err(error) = result {
        panic!("pathon failed to start: {error}");
    }
}
