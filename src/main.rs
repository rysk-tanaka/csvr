use std::{io::BufReader, io::IsTerminal, ops::Range, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, Focusable, FocusHandle, KeyBinding, KeyDownEvent,
    ListHorizontalSizingBehavior, PathBuilder, Render, SharedString, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions, actions, canvas, div, fill, point, prelude::*, px, rgb,
    size, uniform_list,
};

actions!(csvr, [ToggleSearch, DismissSearch, ToggleChart]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChartType {
    Bar,
    Line,
    Scatter,
    Histogram,
}

const CHART_TYPES: [ChartType; 4] = [
    ChartType::Bar,
    ChartType::Line,
    ChartType::Scatter,
    ChartType::Histogram,
];

impl ChartType {
    fn label(self) -> &'static str {
        match self {
            ChartType::Bar => "Bar",
            ChartType::Line => "Line",
            ChartType::Scatter => "Scatter",
            ChartType::Histogram => "Histogram",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

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

/// Determine which columns are numeric based on all rows.
fn compute_numeric_columns(rows: &[Vec<String>], col_count: usize) -> Vec<bool> {
    (0..col_count)
        .map(|col| {
            rows.iter().all(|row| {
                let val = row.get(col).map(|s| s.as_str()).unwrap_or("");
                val.is_empty() || val.parse::<f64>().is_ok()
            })
        })
        .collect()
}

fn sort_indices(
    rows: &[Rc<Vec<String>>],
    indices: &[usize],
    col: usize,
    use_numeric: bool,
    direction: SortDirection,
) -> Vec<usize> {
    if use_numeric {
        // Pre-compute sort keys to avoid O(n log n) parses inside sort_by.
        let mut keyed: Vec<(usize, f64)> = indices
            .iter()
            .map(|&i| {
                let val = rows[i].get(col).map(|s| s.as_str()).unwrap_or("");
                let n = val.parse::<f64>().unwrap_or(f64::NEG_INFINITY);
                (i, n)
            })
            .collect();
        keyed.sort_by(|(_, a), (_, b)| match direction {
            SortDirection::Ascending => a.total_cmp(b),
            SortDirection::Descending => b.total_cmp(a),
        });
        keyed.into_iter().map(|(i, _)| i).collect()
    } else {
        let mut sorted = indices.to_vec();
        sorted.sort_by(|&a, &b| {
            let val_a = rows[a].get(col).map(|s| s.as_str()).unwrap_or("");
            let val_b = rows[b].get(col).map(|s| s.as_str()).unwrap_or("");
            match direction {
                SortDirection::Ascending => val_a.cmp(val_b),
                SortDirection::Descending => val_b.cmp(val_a),
            }
        });
        sorted
    }
}

/// Extract finite numeric values from a specific column for the given row indices.
/// Non-numeric, NaN, Infinity, and missing values are skipped.
fn extract_column_values(
    rows: &[Rc<Vec<String>>],
    indices: &[usize],
    col: usize,
) -> Vec<(usize, f64)> {
    indices
        .iter()
        .filter_map(|&i| {
            let val = rows.get(i)?.get(col)?.as_str();
            val.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| (i, v))
        })
        .collect()
}

/// Downsample a slice to at most `max` points using uniform stride selection.
/// When `max >= 2`, first and last elements are always included to preserve data range boundaries.
/// Returns empty when `max` is 0.
fn downsample<T: Copy>(values: &[T], max: usize) -> Vec<T> {
    if max == 0 {
        return Vec::new();
    }
    if values.len() <= max {
        return values.to_vec();
    }
    if max == 1 {
        return vec![values[0]];
    }
    // Linearly interpolate indices so that first and last are always included
    let last = values.len() - 1;
    (0..max)
        .map(|i| {
            let idx = (i as f64 * last as f64 / (max - 1) as f64).round() as usize;
            values[idx]
        })
        .collect()
}

/// Compute histogram bin counts for the given values.
fn compute_histogram_bins(values: &[f64], bin_count: usize) -> Vec<usize> {
    if values.is_empty() || bin_count == 0 {
        return vec![0; bin_count];
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range == 0.0 {
        // All values identical — put everything in the middle bin
        let mut bins = vec![0; bin_count];
        bins[bin_count / 2] = values.len();
        return bins;
    }
    let mut bins = vec![0; bin_count];
    for &v in values {
        let idx = ((v - min) / range * bin_count as f64) as usize;
        bins[idx.min(bin_count - 1)] += 1;
    }
    bins
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
const CHART_BLUE: u32 = 0x89b4fa;
const CHART_GREEN: u32 = 0xa6e3a1;
const CHART_PEACH: u32 = 0xfab387;
const CHART_MAUVE: u32 = 0xcba6f7;
const SURFACE1: u32 = 0x45475a;
const CHART_CANVAS_HEIGHT: f32 = 280.0;

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
    numeric_columns: Vec<bool>,
    row_num_width: f32,
    scroll_handle: UniformListScrollHandle,
    visible_range: Range<usize>,
    search_active: bool,
    search_query: String,
    filtered_indices: Vec<usize>,
    sort_state: Option<(usize, SortDirection)>,
    chart_active: bool,
    chart_type: ChartType,
    chart_col: usize,
    chart_x_col: usize,
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
        let numeric_columns = compute_numeric_columns(&data.rows, data.headers.len());
        let headers = data
            .headers
            .iter()
            .map(|h| SharedString::from(h.to_uppercase()))
            .collect();
        let total_rows = data.rows.len();
        let rows = data.rows.into_iter().map(Rc::new).collect();
        // Find first two numeric columns for chart defaults
        let first_numeric = numeric_columns.iter().position(|&b| b).unwrap_or(0);
        let second_numeric = numeric_columns
            .iter()
            .skip(first_numeric + 1)
            .position(|&b| b)
            .map(|i| i + first_numeric + 1)
            .unwrap_or(first_numeric);
        Self {
            headers,
            rows,
            col_widths,
            numeric_columns,
            row_num_width,
            scroll_handle: UniformListScrollHandle::new(),
            visible_range: 0..0,
            search_active: false,
            search_query: String::new(),
            filtered_indices: (0..total_rows).collect(),
            sort_state: None,
            chart_active: false,
            chart_type: ChartType::Bar,
            chart_col: first_numeric,
            chart_x_col: second_numeric,
            focus_handle: cx.focus_handle(),
        }
    }

    fn recompute_filtered_indices(&mut self) {
        self.filtered_indices = filter_rows(&self.rows, &self.search_query);
        if let Some((col, direction)) = self.sort_state {
            let use_numeric = self.numeric_columns.get(col).copied().unwrap_or(false);
            self.filtered_indices =
                sort_indices(&self.rows, &self.filtered_indices, col, use_numeric, direction);
        }
    }

    fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        self.recompute_filtered_indices();
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
    }

    fn toggle_sort(&mut self, col: usize) {
        self.sort_state = match self.sort_state {
            Some((c, SortDirection::Ascending)) if c == col => {
                Some((col, SortDirection::Descending))
            }
            Some((c, SortDirection::Descending)) if c == col => None,
            _ => Some((col, SortDirection::Ascending)),
        };
        self.recompute_filtered_indices();
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
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

    fn toggle_chart(&mut self) {
        self.chart_active = !self.chart_active;
    }

    fn set_chart_type(&mut self, ct: ChartType) {
        self.chart_type = ct;
    }

    fn set_chart_col(&mut self, col: usize) {
        if self.numeric_columns.get(col).copied().unwrap_or(false) {
            self.chart_col = col;
        }
    }

    fn set_chart_x_col(&mut self, col: usize) {
        if self.numeric_columns.get(col).copied().unwrap_or(false) {
            self.chart_x_col = col;
        }
    }

    fn numeric_col_indices(&self) -> Vec<usize> {
        self.numeric_columns
            .iter()
            .enumerate()
            .filter(|(_, is_num)| **is_num)
            .map(|(i, _)| i)
            .collect()
    }
}

enum ChartData {
    Points(Vec<f64>),
    Bins(Vec<usize>),
    Pairs(Vec<(f64, f64)>),
}

fn draw_chart(
    window: &mut Window,
    bounds: Bounds<gpui::Pixels>,
    data: &ChartData,
    chart_type: ChartType,
) {
    let padding = 16.0;
    let x0 = bounds.origin.x.0 + padding;
    let y0 = bounds.origin.y.0 + padding;
    let w = bounds.size.width.0 - padding * 2.0;
    let h = bounds.size.height.0 - padding * 2.0;
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    match (chart_type, data) {
        (ChartType::Bar, ChartData::Points(vals)) | (ChartType::Line, ChartData::Points(vals))
            if vals.is_empty() => {}

        (ChartType::Bar, ChartData::Points(vals)) => {
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let base_min = min.min(0.0);
            let effective_range = max - base_min;
            let effective_range = if effective_range.abs() < f64::EPSILON { 1.0 } else { effective_range };

            let n = vals.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &val) in vals.iter().enumerate() {
                let normalized = (val - base_min) / effective_range;
                let bar_h = (normalized as f32 * h).max(1.0);
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_BLUE)));
            }
        }

        (ChartType::Line, ChartData::Points(vals)) => {
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let range = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };

            let n = vals.len();
            let points: Vec<(f32, f32)> = vals
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let px_x = if n > 1 { x0 + i as f32 * w / (n - 1) as f32 } else { x0 + w / 2.0 };
                    let normalized = (v - min) / range;
                    let px_y = y0 + h - normalized as f32 * h;
                    (px_x, px_y)
                })
                .collect();

            // Draw line
            if points.len() >= 2 {
                let mut path = PathBuilder::stroke(px(2.0));
                path.move_to(point(px(points[0].0), px(points[0].1)));
                for &(px_x, px_y) in &points[1..] {
                    path.line_to(point(px(px_x), px(px_y)));
                }
                match path.build() {
                    Ok(path) => window.paint_path(path, rgb(CHART_GREEN)),
                    Err(e) => {
                        eprintln!("Warning: failed to build line chart path: {e:?}");
                    }
                }
            }

            // Draw dots
            let dot_r = 2.5_f32;
            for &(px_x, px_y) in &points {
                let dot = Bounds::new(
                    point(px(px_x - dot_r), px(px_y - dot_r)),
                    size(px(dot_r * 2.0), px(dot_r * 2.0)),
                );
                window.paint_quad(fill(dot, rgb(CHART_GREEN)).corner_radii(px(dot_r)));
            }
        }

        (ChartType::Histogram, ChartData::Bins(bins)) => {
            let max_count = bins.iter().copied().max().unwrap_or(1).max(1);
            let n = bins.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &count) in bins.iter().enumerate() {
                let bar_h = (count as f32 / max_count as f32 * h).max(if count > 0 { 1.0 } else { 0.0 });
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_MAUVE)));
            }
        }

        (ChartType::Scatter, ChartData::Pairs(pairs)) if pairs.is_empty() => {}

        (ChartType::Scatter, ChartData::Pairs(pairs)) => {
            let x_min = pairs.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let x_max = pairs.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let y_min = pairs.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let y_max = pairs.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            let x_range = if (x_max - x_min).abs() < f64::EPSILON { 1.0 } else { x_max - x_min };
            let y_range = if (y_max - y_min).abs() < f64::EPSILON { 1.0 } else { y_max - y_min };

            let dot_r = 3.0_f32;
            for &(xv, yv) in pairs {
                let px_x = x0 + ((xv - x_min) / x_range) as f32 * w;
                let px_y = y0 + h - ((yv - y_min) / y_range) as f32 * h;
                let dot = Bounds::new(
                    point(px(px_x - dot_r), px(px_y - dot_r)),
                    size(px(dot_r * 2.0), px(dot_r * 2.0)),
                );
                window.paint_quad(fill(dot, rgb(CHART_PEACH)).corner_radii(px(dot_r)));
            }
        }

        (ChartType::Bar, ChartData::Bins(_) | ChartData::Pairs(_))
        | (ChartType::Line, ChartData::Bins(_) | ChartData::Pairs(_))
        | (ChartType::Histogram, ChartData::Points(_) | ChartData::Pairs(_))
        | (ChartType::Scatter, ChartData::Points(_) | ChartData::Bins(_)) => {
            debug_assert!(false, "Mismatched ChartType and ChartData");
            eprintln!("Bug: mismatched ChartType and ChartData variant");
        }
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
            .on_action(cx.listener(|this, _: &ToggleChart, _window, cx| {
                this.toggle_chart();
                cx.notify();
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
                            .children(
                                self.headers
                                    .iter()
                                    .zip(self.col_widths.iter())
                                    .enumerate()
                                    .map(|(col_idx, (label, &width))| {
                                        let indicator: SharedString = match self.sort_state {
                                            Some((c, SortDirection::Ascending))
                                                if c == col_idx =>
                                            {
                                                "▲".into()
                                            }
                                            Some((c, SortDirection::Descending))
                                                if c == col_idx =>
                                            {
                                                "▼".into()
                                            }
                                            _ => "".into(),
                                        };
                                        let has_indicator = !indicator.is_empty();
                                        let entity = entity.clone();
                                        div()
                                            .id(("header", col_idx))
                                            .w(px(width))
                                            .flex_shrink_0()
                                            .px_1()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .cursor_pointer()
                                            .on_click(move |_event, _window, cx| {
                                                entity.update(cx, |this, cx| {
                                                    this.toggle_sort(col_idx);
                                                    cx.notify();
                                                });
                                            })
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .truncate()
                                                    .child(label.clone()),
                                            )
                                            .when(has_indicator, |el| {
                                                el.child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .ml_0p5()
                                                        .child(indicator),
                                                )
                                            })
                                    }),
                            ),
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
            // Chart panel
            .when(self.chart_active, |el| {
                let entity = entity.clone();
                let numeric_cols = self.numeric_col_indices();
                let has_numeric = !numeric_cols.is_empty();
                let chart_type = self.chart_type;
                let chart_col = self.chart_col;
                let chart_x_col = self.chart_x_col;
                let headers = &self.headers;

                // Toolbar: chart type buttons + column selectors
                let toolbar = div()
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
                    .children(CHART_TYPES.iter().map(|&ct| {
                        let entity = entity.clone();
                        let is_active = ct == chart_type;
                        div()
                            .id(SharedString::from(format!("chart-type-{}", ct.label())))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .when(is_active, |el| el.bg(rgb(SURFACE1)).text_color(rgb(CHART_BLUE)))
                            .when(!is_active, |el| el.text_color(rgb(TEXT_SUBTEXT)))
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.set_chart_type(ct);
                                    cx.notify();
                                });
                            })
                            .child(ct.label())
                    }))
                    .child(div().w(px(1.0)).h(px(16.0)).bg(rgb(BORDER_COLOR)))
                    .when(has_numeric, |el| {
                        let is_scatter = chart_type == ChartType::Scatter;
                        // Y column (or single column for non-scatter)
                        let col_label: SharedString = if is_scatter {
                            format!("Y: {}", headers.get(chart_col).map(|s| s.as_ref()).unwrap_or("?")).into()
                        } else {
                            headers.get(chart_col).cloned().unwrap_or_else(|| "?".into())
                        };
                        let entity2 = entity.clone();
                        let numeric_cols2 = numeric_cols.clone();
                        el.child(
                            div()
                                .id("chart-col-selector")
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .bg(rgb(SURFACE1))
                                .text_color(rgb(CHART_GREEN))
                                .on_click(move |_, _, cx| {
                                    entity2.update(cx, |this, cx| {
                                        // Cycle to next numeric column
                                        let cur_pos = numeric_cols2.iter().position(|&c| c == this.chart_col).unwrap_or(0);
                                        let next = (cur_pos + 1) % numeric_cols2.len();
                                        this.set_chart_col(numeric_cols2[next]);
                                        cx.notify();
                                    });
                                })
                                .child(col_label),
                        )
                        .when(is_scatter, |el| {
                            let x_label: SharedString = format!("X: {}", headers.get(chart_x_col).map(|s| s.as_ref()).unwrap_or("?")).into();
                            let entity3 = entity.clone();
                            let numeric_cols3 = numeric_cols.clone();
                            el.child(
                                div()
                                    .id("chart-x-col-selector")
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .bg(rgb(SURFACE1))
                                    .text_color(rgb(CHART_PEACH))
                                    .on_click(move |_, _, cx| {
                                        entity3.update(cx, |this, cx| {
                                            let cur_pos = numeric_cols3.iter().position(|&c| c == this.chart_x_col).unwrap_or(0);
                                            let next = (cur_pos + 1) % numeric_cols3.len();
                                            this.set_chart_x_col(numeric_cols3[next]);
                                            cx.notify();
                                        });
                                    })
                                    .child(x_label),
                            )
                        })
                    })
                    .when(!has_numeric, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_SUBTEXT))
                                .child("No numeric columns"),
                        )
                    });

                if !has_numeric || self.filtered_indices.is_empty() {
                    let message = if !has_numeric { "No numeric columns" } else { "No data" };
                    return el.child(toolbar).child(
                        div()
                            .w_full()
                            .h(px(CHART_CANVAS_HEIGHT))
                            .bg(rgb(HEADER_BG))
                            .border_b_1()
                            .border_color(rgb(BORDER_COLOR))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_color(rgb(TEXT_SUBTEXT))
                                    .text_sm()
                                    .child(message),
                            ),
                    );
                }

                // Pre-compute chart data for the canvas closure
                let y_values = extract_column_values(&self.rows, &self.filtered_indices, chart_col);
                let chart_data: ChartData = match chart_type {
                    ChartType::Bar => {
                        let sampled = downsample(&y_values, 100);
                        ChartData::Points(sampled.into_iter().map(|(_, v)| v).collect())
                    }
                    ChartType::Line => {
                        let sampled = downsample(&y_values, 500);
                        ChartData::Points(sampled.into_iter().map(|(_, v)| v).collect())
                    }
                    ChartType::Histogram => {
                        let vals: Vec<f64> = y_values.iter().map(|(_, v)| *v).collect();
                        let bins = compute_histogram_bins(&vals, 30);
                        ChartData::Bins(bins)
                    }
                    ChartType::Scatter => {
                        let x_values = extract_column_values(&self.rows, &self.filtered_indices, chart_x_col);
                        // Match x and y by row index
                        let x_map: std::collections::HashMap<usize, f64> = x_values.into_iter().collect();
                        let pairs: Vec<(f64, f64)> = y_values
                            .iter()
                            .filter_map(|(i, y)| x_map.get(i).map(|x| (*x, *y)))
                            .collect();
                        let limited = downsample(&pairs, 500);
                        ChartData::Pairs(limited)
                    }
                };

                el.child(toolbar).child(
                    div()
                        .w_full()
                        .h(px(CHART_CANVAS_HEIGHT))
                        .bg(rgb(HEADER_BG))
                        .border_b_1()
                        .border_color(rgb(BORDER_COLOR))
                        .child(
                            canvas(
                                move |bounds, _window, _cx| (bounds, chart_data, chart_type),
                                move |_bounds, (bounds, data, ct), window, _cx| {
                                    draw_chart(window, bounds, &data, ct);
                                },
                            )
                            .size_full(),
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

    // --- sort_indices ---

    #[test]
    fn sort_string_ascending() {
        let rows = make_rc_rows(&[&["Charlie"], &["Alice"], &["Bob"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_string_descending() {
        let rows = make_rc_rows(&[&["Charlie"], &["Alice"], &["Bob"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]);
    }

    #[test]
    fn sort_numeric_ascending() {
        let rows = make_rc_rows(&[&["100"], &["3"], &["25"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_numeric_descending() {
        let rows = make_rc_rows(&[&["100"], &["3"], &["25"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]);
    }

    #[test]
    fn sort_respects_filtered_indices() {
        let rows = make_rc_rows(&[&["C"], &["A"], &["B"], &["D"]]);
        let indices = vec![0, 2, 3]; // only rows 0, 2, 3
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![2, 0, 3]);
    }

    #[test]
    fn sort_mixed_numeric_and_string_uses_string_comparison() {
        let rows = make_rc_rows(&[&["banana"], &["10"], &["apple"]]);
        let indices = vec![0, 1, 2];
        // Column has non-numeric values, so entire column uses string comparison
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]); // "10" < "apple" < "banana" (lexicographic)
    }

    #[test]
    fn sort_empty_indices() {
        let rows = make_rc_rows(&[&["A"]]);
        let result = sort_indices(&rows, &[], 0, false, SortDirection::Ascending);
        assert!(result.is_empty());
    }

    #[test]
    fn sort_with_missing_column() {
        let rows = make_rc_rows(&[&["A", "1"], &["B"]]);
        let indices = vec![0, 1];
        // Row 1 has no column 1, treated as numeric (column 1 has "1" and empty)
        let result = sort_indices(&rows, &indices, 1, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 0]);
    }

    #[test]
    fn sort_numeric_with_nan() {
        // "NaN" parses as f64::NAN; total_cmp places NaN after all other values
        let rows = make_rc_rows(&[&["NaN"], &["0"], &["1"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]); // 0 < 1 < NaN
    }

    #[test]
    fn sort_numeric_with_nan_descending() {
        let rows = make_rc_rows(&[&["NaN"], &["0"], &["1"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]); // NaN > 1 > 0
    }

    // --- compute_numeric_columns ---

    #[test]
    fn numeric_columns_detection() {
        let data = make_csv_data(&["name", "age", "score"], &[&["Alice", "30", "95.5"], &["Bob", "25", "87.0"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![false, true, true]);
    }

    #[test]
    fn numeric_columns_with_empty_values() {
        let data = make_csv_data(&["val"], &[&["1"], &[""], &["3"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![true]); // empty values don't disqualify numeric
    }

    #[test]
    fn numeric_columns_mixed() {
        let data = make_csv_data(&["col"], &[&["1"], &["abc"], &["3"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![false]);
    }

    // --- extract_column_values ---

    #[test]
    fn extract_values_basic() {
        let rows = make_rc_rows(&[&["10", "a"], &["20", "b"], &["30", "c"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (1, 20.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_skips_non_numeric() {
        let rows = make_rc_rows(&[&["10"], &["abc"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_respects_indices() {
        let rows = make_rc_rows(&[&["10"], &["20"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_empty_indices() {
        let rows = make_rc_rows(&[&["10"]]);
        let result = extract_column_values(&rows, &[], 0);
        assert!(result.is_empty());
    }

    // --- downsample ---

    #[test]
    fn downsample_no_reduction_needed() {
        let values: Vec<(usize, f64)> = vec![(0, 1.0), (1, 2.0), (2, 3.0)];
        let result = downsample(&values, 5);
        assert_eq!(result, values);
    }

    #[test]
    fn downsample_reduces_to_max() {
        let values: Vec<(usize, f64)> = (0..100).map(|i| (i, i as f64)).collect();
        let result = downsample(&values, 10);
        assert_eq!(result.len(), 10);
        assert_eq!(result[0], (0, 0.0));
        assert_eq!(*result.last().unwrap(), (99, 99.0));
    }

    #[test]
    fn downsample_includes_last_element() {
        let values: Vec<(usize, f64)> = (0..101).map(|i| (i, i as f64)).collect();
        let result = downsample(&values, 100);
        assert_eq!(result.len(), 100);
        assert_eq!(result[0], (0, 0.0));
        assert_eq!(*result.last().unwrap(), (100, 100.0));
    }

    #[test]
    fn downsample_empty() {
        let result = downsample(&[] as &[(usize, f64)], 10);
        assert!(result.is_empty());
    }

    // --- compute_histogram_bins ---

    #[test]
    fn histogram_basic() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let bins = compute_histogram_bins(&values, 5);
        assert_eq!(bins.len(), 5);
        let total: usize = bins.iter().sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn histogram_all_same_value() {
        let values = vec![5.0, 5.0, 5.0];
        let bins = compute_histogram_bins(&values, 4);
        assert_eq!(bins.len(), 4);
        // All values go to the middle bin
        assert_eq!(bins[2], 3);
        assert_eq!(bins.iter().sum::<usize>(), 3);
    }

    #[test]
    fn histogram_empty_values() {
        let bins = compute_histogram_bins(&[], 5);
        assert_eq!(bins, vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn histogram_single_value() {
        let bins = compute_histogram_bins(&[42.0], 3);
        assert_eq!(bins.iter().sum::<usize>(), 1);
    }

    #[test]
    fn histogram_max_value_in_last_bin() {
        let bins = compute_histogram_bins(&[0.0, 5.0, 10.0], 5);
        assert_eq!(bins.len(), 5);
        assert_eq!(bins.iter().sum::<usize>(), 3);
        // max value (10.0) should land in the last bin
        assert!(bins[4] > 0);
    }

    #[test]
    fn histogram_negative_values() {
        let bins = compute_histogram_bins(&[-10.0, -5.0, 0.0, 5.0, 10.0], 5);
        assert_eq!(bins.iter().sum::<usize>(), 5);
    }

    #[test]
    fn histogram_zero_bins() {
        let bins = compute_histogram_bins(&[1.0, 2.0], 0);
        assert!(bins.is_empty());
    }

    // --- downsample edge cases ---

    #[test]
    fn downsample_max_zero_returns_empty() {
        let values: Vec<(usize, f64)> = vec![(0, 1.0), (1, 2.0)];
        let result = downsample(&values, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn downsample_max_one_returns_first() {
        let values: Vec<(usize, f64)> = vec![(0, 1.0), (1, 2.0), (2, 3.0)];
        let result = downsample(&values, 1);
        assert_eq!(result, vec![(0, 1.0)]);
    }

    // --- extract_column_values edge cases ---

    #[test]
    fn extract_values_skips_nan_and_infinity() {
        let rows = make_rc_rows(&[&["10"], &["NaN"], &["inf"], &["-inf"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 1, 2, 3, 4], 0);
        assert_eq!(result, vec![(0, 10.0), (4, 30.0)]);
    }

    #[test]
    fn extract_values_out_of_bounds_index() {
        let rows = make_rc_rows(&[&["10"], &["20"]]);
        let result = extract_column_values(&rows, &[0, 5, 1], 0);
        assert_eq!(result, vec![(0, 10.0), (1, 20.0)]);
    }

    #[test]
    fn extract_values_out_of_bounds_col() {
        let rows = make_rc_rows(&[&["10"], &["20"]]);
        let result = extract_column_values(&rows, &[0, 1], 5);
        assert!(result.is_empty());
    }

    #[test]
    fn extract_values_empty_string() {
        let rows = make_rc_rows(&[&["10"], &[""], &["30"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (2, 30.0)]);
    }

    // --- downsample with (f64, f64) pairs ---

    #[test]
    fn downsample_pairs_no_reduction() {
        let pairs = vec![(1.0, 2.0), (3.0, 4.0)];
        let result = downsample(&pairs, 5);
        assert_eq!(result, pairs);
    }

    #[test]
    fn downsample_pairs_includes_last() {
        let pairs: Vec<(f64, f64)> = (0..101).map(|i| (i as f64, i as f64 * 2.0)).collect();
        let result = downsample(&pairs, 50);
        assert_eq!(result.len(), 50);
        assert_eq!(result[0], (0.0, 0.0));
        assert_eq!(*result.last().unwrap(), (100.0, 200.0));
    }

    #[test]
    fn downsample_pairs_max_zero() {
        let pairs = vec![(1.0, 2.0)];
        let result = downsample(&pairs, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn downsample_pairs_max_one() {
        let pairs = vec![(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)];
        let result = downsample(&pairs, 1);
        assert_eq!(result, vec![(1.0, 2.0)]);
    }

    #[test]
    fn downsample_max_two_returns_first_and_last() {
        let values: Vec<(usize, f64)> = (0..10).map(|i| (i, i as f64)).collect();
        let result = downsample(&values, 2);
        assert_eq!(result, vec![(0, 0.0), (9, 9.0)]);
    }

    #[test]
    fn histogram_skewed_distribution() {
        let values = vec![0.0, 0.1, 0.2, 100.0];
        let bins = compute_histogram_bins(&values, 10);
        // Most values should be in the first bin, only 100.0 in the last
        assert!(bins[0] >= 3);
        assert_eq!(*bins.last().unwrap(), 1);
    }

    #[test]
    fn extract_values_negative_numbers() {
        let rows = make_rc_rows(&[&["-3.14"], &["2.5"], &["-0.001"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, -3.14), (1, 2.5), (2, -0.001)]);
    }
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
