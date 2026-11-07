//! Browser entry point (gpui_web). It shows a built-in sample until a file is dropped on the page.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use gpui::{App, AppContext, AsyncApp, KeyBinding, WindowHandle, WindowOptions};
use gpui_web::{CanvasFontFallback, WebBackendPreference, WebPlatform};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use csvr_core::data::{CsvData, SpreadsheetFormat, decode_to_utf8};

use crate::app::{CsvrApp, OpenFile, ShortcutModifier, key_bindings};
use crate::browser::js_error;

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
        // Only the browser build opens files from the app. GPUI prevents the browser's own
        // shortcut for keys it handles, so the browser's open dialog does not appear too. Unlike
        // the other bindings, these also apply while the IME is composing: otherwise the key
        // would reach the browser, whose dialog opens a picked file in place of the page.
        cx.bind_keys([
            KeyBinding::new("cmd-o", OpenFile, Some("CsvrApp")),
            KeyBinding::new("ctrl-o", OpenFile, Some("CsvrApp")),
        ]);

        let sample = CsvData::from_reader(SAMPLE_CSV.as_bytes()).expect("the sample CSV is valid");
        let window = cx.open_window(WindowOptions::default(), |window, cx| {
            let entity = cx.new(|cx| CsvrApp::new(sample, window, cx));
            let focus = entity.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            entity
        });
        match window {
            Ok(window) => {
                let opener = Rc::new(FileOpener::new(window, cx.to_async()));
                listen_for_drops(Rc::clone(&opener));
                listen_for_file_picker(opener, cx);
            }
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

/// Opens files the user gives the page, by dropping them or with the file picker, in place of
/// the table.
struct FileOpener {
    window: WindowHandle<CsvrApp>,
    cx: AsyncApp,
    /// Reads finish in any order, so only the latest file may replace the table. Otherwise a
    /// large file given first could finish last and cover the file the user picked afterwards.
    latest: Cell<u64>,
}

impl FileOpener {
    fn new(window: WindowHandle<CsvrApp>, cx: AsyncApp) -> Self {
        Self {
            window,
            cx,
            latest: Cell::new(0),
        }
    }

    fn open(self: &Rc<Self>, file: web_sys::File) {
        let id = self.latest.get() + 1;
        self.latest.set(id);
        let opener = Rc::clone(self);
        wasm_bindgen_futures::spawn_local(async move { opener.read_and_show(file, id).await });
    }

    async fn read_and_show(&self, file: web_sys::File, id: u64) {
        let name = file.name();
        let bytes = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
            Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
            Err(e) => {
                self.report_failure(id, format!("Failed to read '{name}': {}", js_error(&e)));
                return;
            }
        };
        // Checked before loading: decoding and parsing run on the only thread, so a file that
        // is already replaced would freeze the page for nothing. `load` has no await, so no
        // newer file can arrive between this check and the update below.
        if self.latest.get() != id {
            log::info!("'{name}' was replaced by a later file before it finished reading");
            return;
        }
        let data = match load(&name, bytes) {
            Ok(data) => data,
            // Keep the current table: a failed file should not leave the page empty.
            Err(e) => {
                self.report_failure(id, format!("Failed to open '{name}': {e}"));
                return;
            }
        };
        let mut cx = self.cx.clone();
        let replaced = self.window.update(&mut cx, |app, window, cx| {
            *app = CsvrApp::new(data, window, cx);
            window.focus(&app.focus_handle, cx);
            cx.notify();
        });
        // Only when the window is gone, so there is no status bar to show it in.
        if let Err(e) = replaced {
            log::error!("failed to show '{name}': {e:#}");
        }
    }

    /// Shows why a file did not open in the status bar, as well as the console: the user picked
    /// or dropped it on the page and would otherwise see nothing happen. A file already replaced
    /// by a later one only goes to the console, so its message does not cover the later file.
    fn report_failure(&self, id: u64, message: String) {
        log::error!("{message}");
        if self.latest.get() != id {
            return;
        }
        let mut cx = self.cx.clone();
        let shown = self
            .window
            .update(&mut cx, |app, _, cx| app.show_status_message(message, cx));
        if let Err(e) = shown {
            log::error!("failed to show the error: {e:#}");
        }
    }
}

/// gpui_web only stops the browser from opening dropped files; it does not hand them to the app,
/// because browsers expose them as `File` objects, not paths. So the page listens for them here.
fn listen_for_drops(opener: Rc<FileOpener>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        log::error!("no document to listen for dropped files");
        return;
    };
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
            opener.open(file);
        });
    let listened =
        document.add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref());
    if let Err(e) = listened {
        log::error!("failed to listen for dropped files: {}", js_error(&e));
        return;
    }
    // The listener lives as long as the page.
    on_drop.forget();
}

/// The files the picker offers by default. The user can still pick any file, which opens as CSV.
/// No `.tsv`: csvr reads only commas, so tab-separated files would load as one column.
const PICKER_ACCEPT: &str = ".csv,.txt,.xlsx,.xls,text/csv";

/// Opens a file picker on `OpenFile` (`cmd-o` / `ctrl-o` or the status bar button). The browser
/// shows a picker only from an `<input type="file">` clicked during a user's input, and GPUI
/// handles the action while the key or click event is still being dispatched.
fn listen_for_file_picker(opener: Rc<FileOpener>, cx: &mut App) {
    let input = match create_file_input() {
        Ok(input) => input,
        Err(e) => {
            log::error!("failed to create the file picker: {e}");
            return;
        }
    };
    let on_change = Closure::<dyn FnMut()>::new({
        let input = input.clone();
        move || {
            // Some browsers fire a change with no files when the picker is cancelled.
            if let Some(file) = input.files().and_then(|files| files.get(0)) {
                opener.open(file);
            }
        }
    });
    let listened =
        input.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref());
    if let Err(e) = listened {
        log::error!("failed to listen for picked files: {}", js_error(&e));
        return;
    }
    // The listener lives as long as the page.
    on_change.forget();
    cx.on_action(move |_: &OpenFile, _| {
        // Cleared so that picking the same file again still fires a change.
        input.set_value("");
        input.click();
    });
}

/// A hidden `<input type="file">` in the document. Some browsers ignore `click()` on an input
/// that is not in the document.
fn create_file_input() -> Result<web_sys::HtmlInputElement, String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let body = document.body().ok_or("no body")?;
    let input: web_sys::HtmlInputElement = document
        .create_element("input")
        .map_err(|e| js_error(&e))?
        .dyn_into()
        .map_err(|_| "not an input element")?;
    input.set_type("file");
    input.set_accept(PICKER_ACCEPT);
    input
        .style()
        .set_property("display", "none")
        .map_err(|e| js_error(&e))?;
    body.append_child(&input).map_err(|e| js_error(&e))?;
    Ok(input)
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
