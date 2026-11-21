// ADR-0014: the `clear` example in the browser, through the thread-free runner.
#![cfg(target_arch = "wasm32")]

use pathon::platform::WindowDesc;
use pathon::rhi::Color;
use pathon::{Renderer, ViewInput, WinitPlatform};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let builder = Renderer::builder().with_clear_color(Color::rgba(0.1, 0.2, 0.35, 1.0));
    let windows = vec![WindowDesc::new("pathon — clear", 960.0, 540.0)];
    let result = pathon::runtime::run(WinitPlatform::new(), builder, windows, |frame| {
        frame
            .windows
            .iter()
            .map(|(target, _)| ViewInput {
                target: *target,
                camera: pathon::api::Camera::default(),
            })
            .collect()
    });
    if let Err(error) = result {
        panic!("pathon failed to start: {error}");
    }
}
