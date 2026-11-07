//! Browser-only helpers shared by the app and the browser entry point. Kept apart from `web` so
//! the app can use them without depending on `web`, which depends on the app.

use std::future::Future;

use wasm_bindgen::{JsCast, JsValue};

/// Starts writing `text` to the clipboard and returns a future with the result. The write
/// starts before this returns, so call it inside the user's input event: the browser only allows
/// it with user activation. The browser refuses outside https and localhost (there is no
/// `navigator.clipboard` there) and when the page lacks permission, and reports that only
/// through the promise, which GPUI's `write_to_clipboard` drops.
pub(crate) fn write_clipboard_text(text: &str) -> impl Future<Output = Result<(), String>> + use<> {
    let promise = start_clipboard_write(text);
    async move {
        let promise = promise?;
        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map(|_| ())
            .map_err(|e| js_error(&e))
    }
}

fn start_clipboard_write(text: &str) -> Result<js_sys::Promise, String> {
    let window = web_sys::window().ok_or("no window")?;
    let navigator = window.navigator();
    // `navigator.clipboard` is undefined outside secure contexts, and calling a method on it
    // would throw.
    let has_clipboard = js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("clipboard"))
        .is_ok_and(|clipboard| !clipboard.is_undefined() && !clipboard.is_null());
    if !has_clipboard {
        return Err("the browser allows copying only on https or localhost".into());
    }
    Ok(navigator.clipboard().write_text(text))
}

/// The message of a JS error, without the stack trace its `Debug` output carries.
pub(crate) fn js_error(value: &JsValue) -> String {
    match value.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => format!("{value:?}"),
    }
}
