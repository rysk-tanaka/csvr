//! Browser entry point (gpui_web). It shows a built-in sample until a file is dropped on the page.

use std::borrow::Cow;
use std::io::Cursor;
use std::rc::Rc;

use gpui::{App, AppContext, AsyncApp, WindowHandle, WindowOptions};
use gpui_web::{CanvasFontFallback, WebBackendPreference, WebPlatform};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use csvr_core::data::{CsvData, SpreadsheetFormat, decode_to_utf8};

use crate::app::{CsvrApp, ShortcutModifier, key_bindings};

const SAMPLE_CSV: &str = "\
名前,部署,年齢,売上
田中,営業,34,1200
佐藤,開発,28,860
鈴木,営業,45,1530
高橋,総務,39,720
伊藤,開発,31,990
";

pub(crate) fn run() {
    gpui_platform::web_init();
    // GPUI starts the browser build with no fonts, and the bundled IBM Plex Sans has no Japanese.
    // EmojiAndCjk lets the browser draw CJK text (gpui_web calls this rendering approximate).
    let platform = WebPlatform::new_with_backend_and_font_fallback(
        false,
        WebBackendPreference::Auto,
        CanvasFontFallback::EmojiAndCjk,
    );
    gpui::Application::with_platform(Rc::new(platform)).run(|cx: &mut App| {
        if let Err(e) = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"
            )),
            // The header is bold; SemiBold is the nearest weight available.
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"
            )),
        ]) {
            log::error!("failed to load fonts: {e:#}");
            return;
        }
        cx.bind_keys(key_bindings(ShortcutModifier::Cmd));
        cx.bind_keys(key_bindings(ShortcutModifier::Ctrl));

        let sample = CsvData::from_reader(SAMPLE_CSV.as_bytes()).expect("the sample CSV is valid");
        let window = cx.open_window(WindowOptions::default(), |window, cx| {
            let entity = cx.new(|cx| CsvrApp::new(sample, cx));
            let focus = entity.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            entity
        });
        match window {
            Ok(window) => listen_for_drops(window, cx.to_async()),
            Err(e) => log::error!("failed to open window: {e:#}"),
        }
    });
}

/// gpui_web only stops the browser from opening dropped files; it does not hand them to the app,
/// because browsers expose them as `File` objects, not paths. So the page listens for them here.
fn listen_for_drops(window: WindowHandle<CsvrApp>, cx: AsyncApp) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        log::error!("no document to listen for dropped files");
        return;
    };
    let on_drop =
        Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |event: web_sys::DragEvent| {
            event.prevent_default();
            let Some(file) = event
                .data_transfer()
                .and_then(|transfer| transfer.files())
                .and_then(|files| files.get(0))
            else {
                return;
            };
            let mut cx = cx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = file.name();
                let bytes = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                    Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                    Err(e) => {
                        log::error!("failed to read '{name}': {e:?}");
                        return;
                    }
                };
                let data = match load(&name, bytes) {
                    Ok(data) => data,
                    // Keep the current table: a failed drop should not leave the page empty.
                    Err(e) => {
                        log::error!("failed to load '{name}': {e}");
                        return;
                    }
                };
                let replaced = window.update(&mut cx, |app, window, cx| {
                    *app = CsvrApp::new(data, cx);
                    window.focus(&app.focus_handle, cx);
                    cx.notify();
                });
                if let Err(e) = replaced {
                    log::error!("failed to show '{name}': {e:#}");
                }
            });
        });
    let listened =
        document.add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref());
    if let Err(e) = listened {
        log::error!("failed to listen for dropped files: {e:?}");
        return;
    }
    // The listener lives as long as the page.
    on_drop.forget();
}

/// The same steps as the native build's stdin path, from bytes instead of a reader.
fn load(name: &str, bytes: Vec<u8>) -> Result<CsvData, String> {
    let format = std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(SpreadsheetFormat::from_extension);
    if let Some(format) = format {
        return CsvData::from_spreadsheet(Cursor::new(bytes), format);
    }
    let (text, decode_warning) = match decode_to_utf8(&bytes) {
        Some(Ok(decoded)) => {
            if let Some(encoding) = decoded.decoded_as {
                log::warn!("'{name}' is not valid UTF-8; decoded as {encoding}");
            }
            (decoded.bytes, decoded.warning)
        }
        Some(Err(e)) => return Err(e),
        None => (bytes, None),
    };
    let mut data = CsvData::from_reader(Cursor::new(text)).map_err(|e| e.to_string())?;
    data.warnings.extend(decode_warning);
    Ok(data)
}
