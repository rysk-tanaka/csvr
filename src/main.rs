mod app;
mod chart;
mod compute;
mod data;

use std::io::Cursor;
use std::io::IsTerminal;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size,
};

use crate::app::{
    CopySelection, CsvrApp, DismissSearch, ExportJson, ExportMarkdown, ToggleChart, ToggleSearch,
};
use crate::data::{CsvData, decode_to_utf8};

fn print_usage_and_exit(msg: &str) -> ! {
    eprintln!("Error: {}", msg);
    eprintln!("Usage: csvr <file.csv|file.xlsx|file.xls>");
    eprintln!("   or: cat file.csv | csvr");
    std::process::exit(1);
}

fn load_csv() -> CsvData {
    let args: Vec<String> = std::env::args().collect();

    // File argument takes priority over stdin
    if args.len() > 2 {
        print_usage_and_exit("too many arguments");
    }
    if args.len() == 2 {
        let path = &args[1];
        let is_spreadsheet = std::path::Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx") || ext.eq_ignore_ascii_case("xls"));

        if is_spreadsheet {
            return CsvData::from_xlsx(path).unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });
        }

        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Error: cannot open '{}': {}", path, e);
            std::process::exit(1);
        });
        // Reuse the original buffer when already UTF-8; allocate a new one only when transcoding is needed
        let transcoded = match decode_to_utf8(&bytes) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            None => bytes,
        };
        return CsvData::from_reader(Cursor::new(transcoded)).unwrap_or_else(|e| {
            eprintln!("Error: failed to parse '{}': {}", path, e);
            eprintln!("Supported formats: .csv, .xlsx, .xls");
            std::process::exit(1);
        });
    }

    // Fall back to stdin when piped
    if !std::io::stdin().is_terminal() {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut bytes).unwrap_or_else(|e| {
            eprintln!("Error: failed to read from stdin: {}", e);
            std::process::exit(1);
        });
        let transcoded = match decode_to_utf8(&bytes) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
            None => bytes,
        };
        return CsvData::from_reader(Cursor::new(transcoded)).unwrap_or_else(|e| {
            eprintln!("Error: failed to parse CSV from stdin: {}", e);
            std::process::exit(1);
        });
    }

    print_usage_and_exit("no input provided");
}

#[allow(unexpected_cfgs)]
fn set_dock_icon() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    static ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

    unsafe {
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes:ICON_PNG.as_ptr() length:ICON_PNG.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
}

fn main() {
    let data = load_csv();
    Application::new().run(|cx: &mut App| {
        set_dock_icon();
        cx.bind_keys([
            KeyBinding::new("cmd-f", ToggleSearch, Some("CsvrApp")),
            KeyBinding::new("escape", DismissSearch, Some("CsvrApp")),
            KeyBinding::new("cmd-g", ToggleChart, Some("CsvrApp")),
            KeyBinding::new("cmd-c", CopySelection, Some("CsvrApp")),
            KeyBinding::new("cmd-shift-j", ExportJson, Some("CsvrApp")),
            KeyBinding::new("cmd-shift-m", ExportMarkdown, Some("CsvrApp")),
        ]);
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let entity = cx.new(|cx| CsvrApp::new(data, cx));
                let focus = entity.read(cx).focus_handle.clone();
                window.focus(&focus);
                entity
            },
        )
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to open window: {}", e);
            eprintln!("Ensure Xcode and Metal are properly installed.");
            std::process::exit(1);
        });
        cx.activate(true);
    });
}
