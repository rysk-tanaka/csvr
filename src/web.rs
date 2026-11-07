//! Browser entry point (gpui_web). It shows a built-in sample until a file is dropped on the page.

use std::borrow::Cow;
use std::cell::Cell;
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

/// A font downloaded when the bundled one lacks a character.
struct FontFile {
    url: &'static str,
    /// Subresource Integrity hash. The browser rejects the response if the content differs, so a
    /// compromised CDN cannot hand the page a different font. It also stands in for checking the
    /// font: `add_fonts` reports success even for bytes it cannot parse, and these bytes are the
    /// files that were checked when the hashes were taken.
    integrity: &'static str,
}

/// IBM Plex Sans JP Regular and SemiBold, the Japanese edition of the bundled IBM Plex Sans. It also
/// has the symbols the bundled font lacks (full-width letters, ①, ㈱, ▲, ...). The npm package
/// only ships WOFF and WOFF2, which cosmic-text cannot read, so the TTFs come from GitHub. The URLs
/// are pinned to the commit of the `@ibm/plex-sans-jp@3.0.0` tag, and the content to its hash.
const IBM_PLEX_SANS_JP: [FontFile; 2] = [
    FontFile {
        url: "https://cdn.jsdelivr.net/gh/IBM/plex@89cba80dad75561262e758f4b6ddd474c5119796/packages/plex-sans-jp/fonts/complete/ttf/unhinted/IBMPlexSansJP-Regular.ttf",
        integrity: "sha384-/yQZjwaTcpaqocYFEcsa32pkb1RuXBjNCLOFAOqOLZ75hLANiYYjiKWMYS7ZOX/4",
    },
    FontFile {
        url: "https://cdn.jsdelivr.net/gh/IBM/plex@89cba80dad75561262e758f4b6ddd474c5119796/packages/plex-sans-jp/fonts/complete/ttf/unhinted/IBMPlexSansJP-SemiBold.ttf",
        integrity: "sha384-HsmSeI1bT3Exy/NcxEvE9gmfB11qUzPrmClQYUGzdkCvMIb0Hr9ct1l2VEhYzjua",
    },
];

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
            // Keep going: CJK text still comes from the browser font, and a blank page would give
            // no hint of what went wrong.
            log::error!("failed to load fonts: {e:#}");
        }
        load_japanese_font_on_missing_glyphs(cx);
        cx.bind_keys(key_bindings(&[
            ShortcutModifier::Cmd,
            ShortcutModifier::Ctrl,
        ]));

        let sample = CsvData::from_reader(SAMPLE_CSV.as_bytes()).expect("the sample CSV is valid");
        let window = cx.open_window(WindowOptions::default(), |window, cx| {
            let entity = cx.new(|cx| CsvrApp::new(sample, window, cx));
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

/// Loads IBM Plex Sans JP the first time some text has a character no font can draw. The two weights
/// together are about 6MB to download, and the sample alone never needs them: gpui_web draws CJK
/// with the browser's font and does not report it as missing.
fn load_japanese_font_on_missing_glyphs(cx: &mut App) {
    // Only one attempt per page. Each new character is reported, and so are characters the
    // Japanese font lacks too, so a failed or useless download would otherwise be repeated.
    let started = Rc::new(Cell::new(false));
    cx.on_missing_glyphs(move |missing, cx| {
        if started.replace(true) {
            return;
        }
        let examples: Vec<&str> = missing
            .iter()
            .take(5)
            .map(|glyph| glyph.grapheme())
            .collect();
        log::info!("no font has {examples:?}; loading IBM Plex Sans JP");
        cx.spawn(async move |cx| load_japanese_font(cx).await)
            .detach();
    })
    // Kept for the life of the page; dropping it would turn the reports off.
    .detach();
}

async fn load_japanese_font(cx: &mut AsyncApp) {
    let [regular, semibold] = &IBM_PLEX_SANS_JP;
    // Both at once, so the tofu stays on screen for one download instead of two. If either fails,
    // neither is added: adding only one weight would mix typefaces between the header and the body.
    let fonts = futures_lite::future::try_zip(fetch_font(regular), fetch_font(semibold)).await;
    let (regular, semibold) = match fonts {
        Ok(fonts) => fonts,
        Err(e) => {
            log::error!("{e}");
            return;
        }
    };
    cx.update(|cx| {
        let fonts = vec![Cow::Owned(regular), Cow::Owned(semibold)];
        if let Err(e) = cx.text_system().add_fonts(fonts) {
            log::error!("failed to add IBM Plex Sans JP: {e:#}");
            return;
        }
        // The column widths were measured with the missing glyphs' width.
        for window in cx.windows() {
            let Some(window) = window.downcast::<CsvrApp>() else {
                continue;
            };
            if let Err(e) = window.update(cx, |app, window, _| app.remeasure_columns(window)) {
                log::error!("failed to measure the columns again: {e:#}");
            }
        }
        // Text already on screen keeps its old layout until it is drawn again.
        cx.refresh_windows();
        log::info!("loaded IBM Plex Sans JP");
    });
}

async fn fetch_font(font: &FontFile) -> Result<Vec<u8>, String> {
    fetch_bytes(font.url, font.integrity)
        .await
        .map_err(|e| format!("failed to load '{}': {e}", font.url))
}

async fn fetch_bytes(url: &str, integrity: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window to fetch from")?;
    let init = web_sys::RequestInit::new();
    init.set_integrity(integrity);
    // The browser gives the same error for a network failure and an integrity mismatch.
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(|e| format!("network error or integrity mismatch: {}", js_error(&e)))?;
    let response: web_sys::Response = response
        .dyn_into()
        .map_err(|e| format!("not a response: {}", js_error(&e)))?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let buffer = response.array_buffer().map_err(|e| js_error(&e))?;
    let buffer = wasm_bindgen_futures::JsFuture::from(buffer)
        .await
        .map_err(|e| js_error(&e))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// The message of a JS error, without the stack trace its `Debug` output carries.
fn js_error(value: &wasm_bindgen::JsValue) -> String {
    match value.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => format!("{value:?}"),
    }
}

/// gpui_web only stops the browser from opening dropped files; it does not hand them to the app,
/// because browsers expose them as `File` objects, not paths. So the page listens for them here.
fn listen_for_drops(window: WindowHandle<CsvrApp>, cx: AsyncApp) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        log::error!("no document to listen for dropped files");
        return;
    };
    // Reads finish in any order, so only the latest drop may replace the table. Otherwise a large
    // file dropped first could finish last and cover the file the user picked afterwards.
    let latest_drop = Rc::new(Cell::new(0u64));
    let on_drop =
        Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |event: web_sys::DragEvent| {
            event.prevent_default();
            let Some(files) = event.data_transfer().and_then(|transfer| transfer.files()) else {
                return;
            };
            let Some(file) = files.get(0) else {
                log::warn!("the drop has no files");
                return;
            };
            if files.length() > 1 {
                log::warn!("{} files dropped; only the first is opened", files.length());
            }
            let drop_id = latest_drop.get() + 1;
            latest_drop.set(drop_id);
            let latest_drop = Rc::clone(&latest_drop);
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
                // Checked before loading: decoding and parsing run on the only thread, so a file
                // that is already replaced would freeze the page for nothing. `load` has no await,
                // so no newer drop can arrive between this check and the update below.
                if latest_drop.get() != drop_id {
                    log::info!("'{name}' was replaced by a later drop before it finished reading");
                    return;
                }
                let data = match load(&name, bytes) {
                    Ok(data) => data,
                    // Keep the current table: a failed drop should not leave the page empty.
                    Err(e) => {
                        log::error!("failed to load '{name}': {e}");
                        return;
                    }
                };
                let replaced = window.update(&mut cx, |app, window, cx| {
                    *app = CsvrApp::new(data, window, cx);
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

/// The same steps as the native build's file path, from bytes instead of a file.
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
            // Free the original before parsing, as the native build does: wasm memory is capped,
            // and the original, the transcoded copy and the table would otherwise coexist.
            drop(bytes);
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
