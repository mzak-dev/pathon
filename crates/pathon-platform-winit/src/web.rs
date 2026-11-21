use pathon_platform::{PlatformError, WindowDesc};
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlElement};

const CONTAINER_ID: &str = "pathon";

fn failure(message: &str) -> PlatformError {
    PlatformError::WindowCreation(message.to_owned())
}

fn container() -> Result<HtmlElement, PlatformError> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| failure("there is no document to attach the canvas to"))?;
    match document.get_element_by_id(CONTAINER_ID) {
        Some(element) => element
            .dyn_into::<HtmlElement>()
            .map_err(|_| failure("the #pathon element is not an HTML element")),
        None => document.body().ok_or_else(|| failure("the document has no body")),
    }
}

// The canvas is attached here, not by winit, so its backing store already has the container's size when the runner
// reads `inner_size` (the ResizeObserver of winit only reports later). An unsized container falls back to `desc`.
pub(crate) fn create_canvas(desc: &WindowDesc) -> Result<HtmlCanvasElement, PlatformError> {
    let container = container()?;
    let canvas = container
        .owner_document()
        .ok_or_else(|| failure("the container has no document"))?
        .create_element("canvas")
        .map_err(|_| failure("cannot create a canvas element"))?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| failure("a canvas element is not an HtmlCanvasElement"))?;
    let style = canvas.style();
    for (property, value) in [("width", "100%"), ("height", "100%"), ("display", "block")] {
        style
            .set_property(property, value)
            .map_err(|_| failure("cannot style the canvas"))?;
    }
    container
        .append_child(&canvas)
        .map_err(|_| failure("cannot attach the canvas to the page"))?;

    let scale = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
    let measured = (f64::from(canvas.client_width()), f64::from(canvas.client_height()));
    let (width, height) = if measured.0 > 0.0 && measured.1 > 0.0 {
        measured
    } else {
        (desc.size.width, desc.size.height)
    };
    canvas.set_width(physical(width, scale));
    canvas.set_height(physical(height, scale));
    Ok(canvas)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to at least 1 above"
)]
fn physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().max(1.0) as u32
}
