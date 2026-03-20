mod app;
mod chart;
mod compute;
mod data;

use std::io::BufReader;
use std::io::IsTerminal;

use gpui::{App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};

use crate::app::{CsvrApp, DismissSearch, ToggleChart, ToggleSearch};
use crate::data::CsvData;

fn print_usage_and_exit(msg: &str) -> ! {
    eprintln!("Error: {}", msg);
    eprintln!("Usage: csvr <file.csv>");
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
        let file = std::fs::File::open(path).unwrap_or_else(|e| {
            eprintln!("Error: cannot open '{}': {}", path, e);
            std::process::exit(1);
        });
        return CsvData::from_reader(file).unwrap_or_else(|e| {
            eprintln!("Error: failed to parse CSV '{}': {}", path, e);
            std::process::exit(1);
        });
    }

    // Fall back to stdin when piped (BufReader streams without loading entire input into memory)
    if !std::io::stdin().is_terminal() {
        let reader = BufReader::new(std::io::stdin().lock());
        return CsvData::from_reader(reader).unwrap_or_else(|e| {
            eprintln!("Error: failed to parse CSV from stdin: {}", e);
            std::process::exit(1);
        });
    }

    print_usage_and_exit("no input provided");
}

fn main() {
    let data = load_csv();
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-f", ToggleSearch, Some("CsvrApp")),
            KeyBinding::new("escape", DismissSearch, Some("CsvrApp")),
            KeyBinding::new("cmd-g", ToggleChart, Some("CsvrApp")),
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
