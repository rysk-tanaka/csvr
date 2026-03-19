use std::{io::BufReader, io::IsTerminal, ops::Range, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, Focusable, FocusHandle, KeyBinding, KeyDownEvent,
    ListHorizontalSizingBehavior, Render, SharedString, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size, uniform_list,
};

actions!(csvr, [ToggleSearch, DismissSearch]);

struct CsvData {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl CsvData {
    fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, csv::Error> {
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(reader);
        let headers = rdr.headers()?.iter().map(|s| s.to_string()).collect();
        let rows = rdr
            .records()
            .map(|r| r.map(|record| record.iter().map(|s| s.to_string()).collect()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CsvData { headers, rows })
    }
}

/// Pixel width per character (monospace approximation at text_sm)
const CHAR_WIDTH: f32 = 7.5;
const MIN_COL_WIDTH: f32 = 50.0;
const MAX_COL_WIDTH: f32 = 400.0;
const COL_PADDING: f32 = 24.0;
const ROW_NUM_WIDTH: f32 = 16.0;

fn compute_column_widths(data: &CsvData) -> Vec<f32> {
    let sample_count = data.rows.len().min(100);
    data.headers
        .iter()
        .enumerate()
        .map(|(col_idx, header)| {
            let max_len = data.rows[..sample_count]
                .iter()
                .map(|row| row.get(col_idx).map_or(0, |cell| cell.chars().count()))
                .max()
                .unwrap_or(0)
                .max(header.to_uppercase().chars().count());
            (max_len as f32 * CHAR_WIDTH + COL_PADDING)
                .clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
        })
        .collect()
}

fn row_number_col_width(total_rows: usize) -> f32 {
    let digits = total_rows.max(1).ilog10() as usize + 1;
    (digits as f32 * CHAR_WIDTH + ROW_NUM_WIDTH).max(40.0)
}

fn filter_rows(rows: &[Rc<Vec<String>>], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..rows.len()).collect();
    }
    let query_lower = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            row.iter()
                .any(|cell| cell.to_lowercase().contains(&query_lower))
        })
        .map(|(i, _)| i)
        .collect()
}

// Catppuccin Mocha palette
const BG_BASE: u32 = 0x1e1e2e;
const TEXT_MAIN: u32 = 0xcdd6f4;
const TEXT_SUBTEXT: u32 = 0xa6adc8;
const BORDER_COLOR: u32 = 0x45475a;
const HEADER_BG: u32 = 0x181825;
const ROW_ALT_BG: u32 = 0x1e1e2e; // Base
const ROW_EVEN_BG: u32 = 0x11111b; // Crust (darker for contrast)
const SEARCH_BG: u32 = 0x313244; // Surface0

#[derive(IntoElement)]
struct TableRow {
    ix: usize,
    row_num: usize,
    cells: Rc<Vec<String>>,
    col_widths: Rc<Vec<f32>>,
    row_num_width: f32,
}

impl RenderOnce for TableRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bg = if self.ix.is_multiple_of(2) {
            ROW_EVEN_BG
        } else {
            ROW_ALT_BG
        };

        div()
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(rgb(BORDER_COLOR))
            .bg(rgb(bg))
            .py_0p5()
            .child(
                div()
                    .w(px(self.row_num_width))
                    .flex_shrink_0()
                    .px_1()
                    .text_color(rgb(TEXT_SUBTEXT))
                    .text_right()
                    .child(self.row_num.to_string()),
            )
            .children(
                self.col_widths
                    .iter()
                    .enumerate()
                    .map(|(col_idx, &width)| {
                        let text: SharedString = self
                            .cells
                            .get(col_idx)
                            .cloned()
                            .unwrap_or_default()
                            .into();
                        div()
                            .w(px(width))
                            .flex_shrink_0()
                            .px_1()
                            .whitespace_nowrap()
                            .truncate()
                            .child(text)
                    }),
            )
    }
}

struct CsvrApp {
    headers: Vec<SharedString>,
    rows: Vec<Rc<Vec<String>>>,
    col_widths: Rc<Vec<f32>>,
    row_num_width: f32,
    scroll_handle: UniformListScrollHandle,
    visible_range: Range<usize>,
    search_active: bool,
    search_query: String,
    filtered_indices: Vec<usize>,
    focus_handle: FocusHandle,
}

impl Focusable for CsvrApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CsvrApp {
    // HACK: GPUI has no public API for horizontal scroll offset on UniformListScrollHandle.
    // Access internal fields directly. Replace when a public API becomes available.
    fn h_scroll_offset(&self) -> gpui::Pixels {
        self.scroll_handle.0.borrow().base_handle.offset().x
    }

    fn new(data: CsvData, cx: &mut Context<Self>) -> Self {
        let col_widths = Rc::new(compute_column_widths(&data));
        let row_num_width = row_number_col_width(data.rows.len());
        let headers = data
            .headers
            .iter()
            .map(|h| SharedString::from(h.to_uppercase()))
            .collect();
        let total_rows = data.rows.len();
        let rows = data.rows.into_iter().map(Rc::new).collect();
        Self {
            headers,
            rows,
            col_widths,
            row_num_width,
            scroll_handle: UniformListScrollHandle::new(),
            visible_range: 0..0,
            search_active: false,
            search_query: String::new(),
            filtered_indices: (0..total_rows).collect(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        self.filtered_indices = filter_rows(&self.rows, &self.search_query);
        self.scroll_handle.scroll_to_item(0, gpui::ScrollStrategy::Top);
    }

    fn toggle_search(&mut self) {
        if self.search_active {
            self.close_search();
        } else {
            self.search_active = true;
        }
    }

    fn close_search(&mut self) {
        self.search_active = false;
        self.set_search_query(String::new());
    }
}

impl Render for CsvrApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let h_offset = self.h_scroll_offset();
        let filtered_count = self.filtered_indices.len();
        let total_count = self.rows.len();

        div()
            .track_focus(&self.focus_handle(cx))
            .key_context("CsvrApp")
            .on_action(cx.listener(|this, _: &ToggleSearch, _window, cx| {
                this.toggle_search();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DismissSearch, _window, cx| {
                if this.search_active {
                    this.close_search();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                // `/` opens search only when inactive
                if !this.search_active && keystroke.key_char.as_deref() == Some("/") {
                    this.search_active = true;
                    cx.notify();
                    return;
                }

                if !this.search_active {
                    return;
                }

                // Ignore modifier combos (Cmd+C, etc.)
                if keystroke.modifiers.platform || keystroke.modifiers.control {
                    return;
                }

                if keystroke.key == "backspace" {
                    let mut q = this.search_query.clone();
                    q.pop();
                    this.set_search_query(q);
                    cx.notify();
                } else if let Some(ch) = &keystroke.key_char {
                    let mut q = this.search_query.clone();
                    q.push_str(ch);
                    this.set_search_query(q);
                    cx.notify();
                }
            }))
            .font_family(".SystemUIFont")
            .bg(rgb(BG_BASE))
            .text_color(rgb(TEXT_MAIN))
            .text_sm()
            .size_full()
            .flex()
            .flex_col()
            // Header (outer container keeps background/border fixed)
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(rgb(BORDER_COLOR))
                    .bg(rgb(HEADER_BG))
                    .text_color(rgb(TEXT_SUBTEXT))
                    .py_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    // Inner row shifts with horizontal scroll offset
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .ml(h_offset)
                            .child(
                                div()
                                    .w(px(self.row_num_width))
                                    .flex_shrink_0()
                                    .px_1()
                                    .text_right()
                                    .child("#"),
                            )
                            .children(self.headers.iter().zip(self.col_widths.iter()).map(
                                |(label, &width)| {
                                    div()
                                        .w(px(width))
                                        .flex_shrink_0()
                                        .px_1()
                                        .whitespace_nowrap()
                                        .truncate()
                                        .child(label.clone())
                                },
                            )),
                    ),
            )
            // Search bar
            .when(self.search_active, |el| {
                let query_display: SharedString = if self.search_query.is_empty() {
                    "Type to search...".into()
                } else {
                    self.search_query.clone().into()
                };
                let query_color = if self.search_query.is_empty() {
                    TEXT_SUBTEXT
                } else {
                    TEXT_MAIN
                };
                el.child(
                    div()
                        .w_full()
                        .px_2()
                        .py_1()
                        .bg(rgb(SEARCH_BG))
                        .border_b_1()
                        .border_color(rgb(BORDER_COLOR))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_color(rgb(TEXT_SUBTEXT))
                                .text_xs()
                                .child("/"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(rgb(query_color))
                                .child(query_display),
                        )
                        .child(
                            div()
                                .text_color(rgb(TEXT_SUBTEXT))
                                .text_xs()
                                .child(format!("{} / {} rows", filtered_count, total_count)),
                        ),
                )
            })
            // Body
            .child(
                div().flex_1().size_full().child(
                    uniform_list(entity, "rows", filtered_count, {
                        move |this, range, _, _| {
                            this.visible_range = range.clone();
                            range
                                .filter_map(|i| {
                                    let original_idx = *this.filtered_indices.get(i)?;
                                    Some(TableRow {
                                        ix: i,
                                        row_num: original_idx + 1,
                                        cells: this.rows.get(original_idx)?.clone(),
                                        col_widths: this.col_widths.clone(),
                                        row_num_width: this.row_num_width,
                                    })
                                })
                                .collect()
                        }
                    })
                    .with_horizontal_sizing_behavior(
                        ListHorizontalSizingBehavior::Unconstrained,
                    )
                    .size_full()
                    .track_scroll(self.scroll_handle.clone()),
                ),
            )
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_csv_data(headers: &[&str], rows: &[&[&str]]) -> CsvData {
        CsvData {
            headers: headers.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|s| s.to_string()).collect())
                .collect(),
        }
    }

    fn make_rc_rows(rows: &[&[&str]]) -> Vec<Rc<Vec<String>>> {
        rows.iter()
            .map(|row| Rc::new(row.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    // --- CsvData::from_reader ---

    #[test]
    fn parse_basic_csv() {
        let input = "name,age,city\nAlice,30,Tokyo\nBob,25,Osaka\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["name", "age", "city"]);
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.rows[0], vec!["Alice", "30", "Tokyo"]);
        assert_eq!(data.rows[1], vec!["Bob", "25", "Osaka"]);
    }

    #[test]
    fn parse_single_column() {
        let input = "value\n1\n2\n3\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["value"]);
        assert_eq!(data.rows.len(), 3);
    }

    #[test]
    fn parse_empty_body() {
        let input = "a,b,c\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["a", "b", "c"]);
        assert!(data.rows.is_empty());
    }

    #[test]
    fn parse_quoted_fields() {
        let input = "name,note\nAlice,\"hello, world\"\nBob,\"line1\nline2\"\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.rows[0][1], "hello, world");
        assert_eq!(data.rows[1][1], "line1\nline2");
    }

    #[test]
    fn parse_empty_input() {
        let input = "";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert!(data.headers.is_empty());
        assert!(data.rows.is_empty());
    }

    // --- compute_column_widths ---

    #[test]
    fn column_widths_basic() {
        let data = make_csv_data(&["name", "age"], &[&["Alice", "30"], &["Bob", "25"]]);
        let widths = compute_column_widths(&data);
        assert_eq!(widths.len(), 2);
        // "Alice" (5 chars) > "name" (4 chars) => 5 * 7.5 + 24 = 61.5
        assert!((widths[0] - 61.5).abs() < 0.01);
        // "age" (3 chars) > "30" (2 chars) => 3 * 7.5 + 24 = 46.5 => clamped to MIN_COL_WIDTH
        assert!((widths[1] - MIN_COL_WIDTH).abs() < 0.01);
    }

    #[test]
    fn column_widths_clamp_to_max() {
        let long_value = "x".repeat(100);
        let data = make_csv_data(&["col"], &[&[&long_value]]);
        let widths = compute_column_widths(&data);
        assert!((widths[0] - MAX_COL_WIDTH).abs() < 0.01);
    }

    #[test]
    fn column_widths_empty_rows() {
        let data = make_csv_data(&["name", "age"], &[]);
        let widths = compute_column_widths(&data);
        // header "name" (4 chars) => 4 * 7.5 + 24 = 54.0
        assert!((widths[0] - 54.0).abs() < 0.01);
    }

    #[test]
    fn column_widths_header_longer_than_data() {
        let data = make_csv_data(&["description"], &[&["hi"]]);
        let widths = compute_column_widths(&data);
        // "description" (11 chars) > "hi" (2 chars) => 11 * 7.5 + 24 = 106.5
        assert!((widths[0] - 106.5).abs() < 0.01);
    }

    #[test]
    fn parse_ragged_rows() {
        let input = "a,b,c\n1,2\n4,5,6\n";
        let result = CsvData::from_reader(input.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn column_widths_short_rows() {
        let data = make_csv_data(&["a", "b", "c"], &[&["x", "y"]]);
        let widths = compute_column_widths(&data);
        assert_eq!(widths.len(), 3);
        // 3rd column: header "c" (1 char) only => 1 * 7.5 + 24 = 31.5 => MIN_COL_WIDTH
        assert!((widths[2] - MIN_COL_WIDTH).abs() < 0.01);
    }

    // --- row_number_col_width ---

    #[test]
    fn row_num_width_zero_rows() {
        let w = row_number_col_width(0);
        // 1 digit => 1 * 7.5 + 16 = 23.5 => clamped to 40.0
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_single_digit() {
        let w = row_number_col_width(9);
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_boundary_ten() {
        let w = row_number_col_width(10);
        // 2 digits => 2 * 7.5 + 16 = 31.0 => clamped to 40.0
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_two_digits() {
        let w = row_number_col_width(99);
        // 2 digits => 2 * 7.5 + 16 = 31.0 => clamped to 40.0
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_four_digits() {
        let w = row_number_col_width(9999);
        // 4 digits => 4 * 7.5 + 16 = 46.0
        assert!((w - 46.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_large() {
        let w = row_number_col_width(1_000_000);
        // 7 digits => 7 * 7.5 + 16 = 68.5
        assert!((w - 68.5).abs() < 0.01);
    }

    // --- filter_rows ---

    #[test]
    fn filter_empty_query_returns_all() {
        let rows = make_rc_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        assert_eq!(filter_rows(&rows, ""), vec![0, 1]);
    }

    #[test]
    fn filter_matches_substring() {
        let rows = make_rc_rows(&[&["Alice", "Tokyo"], &["Bob", "Osaka"], &["Carol", "Tokyo"]]);
        assert_eq!(filter_rows(&rows, "tokyo"), vec![0, 2]);
    }

    #[test]
    fn filter_case_insensitive() {
        let rows = make_rc_rows(&[&["ALICE"], &["alice"], &["Alice"]]);
        assert_eq!(filter_rows(&rows, "alice"), vec![0, 1, 2]);
    }

    #[test]
    fn filter_no_matches() {
        let rows = make_rc_rows(&[&["Alice"], &["Bob"]]);
        let result = filter_rows(&rows, "xyz");
        assert!(result.is_empty());
    }

    #[test]
    fn filter_matches_any_column() {
        let rows = make_rc_rows(&[&["Alice", "30", "Tokyo"], &["Bob", "25", "Osaka"]]);
        assert_eq!(filter_rows(&rows, "25"), vec![1]);
    }

    #[test]
    fn filter_empty_rows() {
        let rows: Vec<Rc<Vec<String>>> = vec![];
        assert!(filter_rows(&rows, "test").is_empty());
    }
}

fn main() {
    let data = load_csv();
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-f", ToggleSearch, Some("CsvrApp")),
            KeyBinding::new("escape", DismissSearch, Some("CsvrApp")),
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
