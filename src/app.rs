use std::{ops::Range, rc::Rc};

use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, FocusHandle, KeyDownEvent,
    ListHorizontalSizingBehavior, Render, SharedString, UniformListScrollHandle,
    Window, actions, canvas, div, prelude::*, px, rgb,
    uniform_list,
};

use crate::chart::{
    CHART_BLUE, CHART_CANVAS_HEIGHT, CHART_GREEN, CHART_PEACH, draw_chart,
};
use crate::compute::{
    compute_column_stats, compute_column_widths, compute_numeric_columns, compute_histogram_bins,
    downsample, extract_column_values, extract_scatter_pairs, filter_rows, row_number_col_width,
    sort_indices,
};
use crate::data::{
    CHART_TYPES, ChartData, ChartType, CsvData, SortDirection,
};

actions!(csvr, [ToggleSearch, DismissSearch, ToggleChart, CopySelection]);

// Catppuccin Mocha palette
const BG_BASE: u32 = 0x1e1e2e;
const TEXT_MAIN: u32 = 0xcdd6f4;
const TEXT_SUBTEXT: u32 = 0xa6adc8;
const BORDER_COLOR: u32 = 0x45475a;
const HEADER_BG: u32 = 0x181825;
const ROW_ALT_BG: u32 = 0x1e1e2e; // Base
const ROW_EVEN_BG: u32 = 0x11111b; // Crust (darker for contrast)
const SEARCH_BG: u32 = 0x313244; // Surface0
const SURFACE1: u32 = 0x45475a;
const ROW_HOVER_BG: u32 = 0x27273a; // Between Base and Surface0 — hover
const ROW_SELECTED_BG: u32 = 0x313244; // Surface0 — selected row
const CELL_SELECTED_BG: u32 = 0x45475a; // Surface1 — selected cell
const STATUS_BG: u32 = 0x181825; // Mantle — status bar background

#[derive(IntoElement)]
struct TableRow {
    ix: usize,
    row_num: usize,
    cells: Rc<Vec<String>>,
    col_widths: Rc<Vec<f32>>,
    row_num_width: f32,
    min_row_width: gpui::Pixels,
    /// None = not selected, Some(None) = entire row selected, Some(Some(c)) = cell c selected
    selected_col: Option<Option<usize>>,
    entity: Entity<CsvrApp>,
}

impl RenderOnce for TableRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let is_row_selected = self.selected_col.is_some();
        let bg = if is_row_selected {
            ROW_SELECTED_BG
        } else if self.ix.is_multiple_of(2) {
            ROW_EVEN_BG
        } else {
            ROW_ALT_BG
        };

        let entity = self.entity.clone();
        let ix = self.ix;
        let selected_col = self.selected_col;

        div()
            .id(("row", ix))
            .flex()
            .flex_row()
            .min_w(self.min_row_width)
            .border_b_1()
            .border_color(rgb(BORDER_COLOR))
            .bg(rgb(bg))
            .when(!is_row_selected, |el| {
                el.hover(|style| style.bg(rgb(ROW_HOVER_BG)))
            })
            .py_0p5()
            .child({
                let entity = entity.clone();
                div()
                    .id(("row-num", ix))
                    .w(px(self.row_num_width))
                    .flex_shrink_0()
                    .px_1()
                    .text_color(rgb(TEXT_SUBTEXT))
                    .text_right()
                    .cursor_pointer()
                    .on_click(move |_event, _window, cx| {
                        entity.update(cx, |this, cx| {
                            this.select_cell(ix, None);
                            cx.notify();
                        });
                    })
                    .child(self.row_num.to_string())
            })
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
                        let is_cell_selected =
                            matches!(selected_col, Some(Some(c)) if c == col_idx);
                        let entity = entity.clone();
                        div()
                            .id(SharedString::from(format!("cell-{}-{}", ix, col_idx)))
                            .w(px(width))
                            .flex_shrink_0()
                            .px_1()
                            .whitespace_nowrap()
                            .truncate()
                            .when(is_cell_selected, |el| el.bg(rgb(CELL_SELECTED_BG)))
                            .on_click(move |_event, _window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.select_cell(ix, Some(col_idx));
                                    cx.notify();
                                });
                            })
                            .child(text)
                    }),
            )
    }
}

pub(crate) struct CsvrApp {
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
    chart_data_cache: Option<ChartData>,
    /// Selected cell: (filtered_index, column). column=None means entire row.
    selected_cell: Option<(usize, Option<usize>)>,
    pub(crate) focus_handle: FocusHandle,
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

    pub(crate) fn new(data: CsvData, cx: &mut Context<Self>) -> Self {
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
            chart_data_cache: None,
            selected_cell: None,
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
        self.selected_cell = None;
        if self.chart_active {
            self.recompute_chart_data();
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
        if self.chart_active {
            self.recompute_chart_data();
        }
    }

    fn set_chart_type(&mut self, ct: ChartType) {
        self.chart_type = ct;
        self.recompute_chart_data();
    }

    fn set_chart_col(&mut self, col: usize) {
        if self.numeric_columns.get(col).copied().unwrap_or(false) {
            self.chart_col = col;
            self.recompute_chart_data();
        }
    }

    fn set_chart_x_col(&mut self, col: usize) {
        if self.numeric_columns.get(col).copied().unwrap_or(false) {
            self.chart_x_col = col;
            self.recompute_chart_data();
        }
    }

    fn recompute_chart_data(&mut self) {
        if !self.chart_active || self.filtered_indices.is_empty() {
            self.chart_data_cache = None;
            return;
        }
        let has_numeric = self.numeric_columns.iter().any(|&b| b);
        if !has_numeric {
            self.chart_data_cache = None;
            return;
        }
        let y_values = extract_column_values(&self.rows, &self.filtered_indices, self.chart_col);
        self.chart_data_cache = Some(match self.chart_type {
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
                let pairs = extract_scatter_pairs(&self.rows, &self.filtered_indices, self.chart_x_col, self.chart_col);
                let limited = downsample(&pairs, 500);
                ChartData::Pairs(limited)
            }
        });
    }

    fn select_cell(&mut self, filtered_idx: usize, col: Option<usize>) {
        if self.filtered_indices.is_empty() {
            self.clear_selection();
            return;
        }
        let clamped = filtered_idx.min(self.filtered_indices.len() - 1);
        self.selected_cell = Some((clamped, col));
    }

    fn clear_selection(&mut self) {
        self.selected_cell = None;
    }

    fn move_selection(&mut self, row_delta: isize, col_delta: isize) {
        let row_count = self.filtered_indices.len();
        if row_count == 0 {
            return;
        }
        let col_count = self.headers.len();

        let (row, col) = match self.selected_cell {
            Some((r, c)) => (r, c),
            None => {
                let initial_col = if col_count > 0 { Some(0) } else { None };
                self.selected_cell = Some((0, initial_col));
                self.ensure_visible(0);
                return;
            }
        };

        let new_row = (row as isize + row_delta).clamp(0, row_count as isize - 1) as usize;

        let new_col = match col {
            Some(c) if col_count > 0 => {
                let new_c = (c as isize + col_delta).clamp(0, col_count as isize - 1) as usize;
                Some(new_c)
            }
            _ => {
                if col_delta > 0 && col_count > 0 {
                    Some(0)
                } else {
                    None
                }
            }
        };

        self.selected_cell = Some((new_row, new_col));
        self.ensure_visible(new_row);
    }

    fn ensure_visible(&self, filtered_idx: usize) {
        if filtered_idx < self.visible_range.start || filtered_idx >= self.visible_range.end {
            self.scroll_handle
                .scroll_to_item(filtered_idx, gpui::ScrollStrategy::Center);
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        let Some((filtered_idx, col)) = self.selected_cell else {
            return;
        };
        let Some(&original_idx) = self.filtered_indices.get(filtered_idx) else {
            return;
        };
        let Some(row) = self.rows.get(original_idx) else {
            return;
        };

        let text = match col {
            Some(c) => row.get(c).cloned().unwrap_or_default(),
            None => row.join("\t"),
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));
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

impl Render for CsvrApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let h_offset = self.h_scroll_offset();
        let filtered_count = self.filtered_indices.len();
        let total_count = self.rows.len();
        let viewport_width = window.viewport_size().width;

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
                } else if this.selected_cell.is_some() {
                    this.clear_selection();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleChart, _window, cx| {
                this.toggle_chart();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _window, cx| {
                this.copy_selection(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;

                // Arrow key navigation (works regardless of search state)
                if !keystroke.modifiers.platform && !keystroke.modifiers.control {
                    match keystroke.key.as_str() {
                        "up" => {
                            this.move_selection(-1, 0);
                            cx.notify();
                            return;
                        }
                        "down" => {
                            this.move_selection(1, 0);
                            cx.notify();
                            return;
                        }
                        "left" => {
                            this.move_selection(0, -1);
                            cx.notify();
                            return;
                        }
                        "right" => {
                            this.move_selection(0, 1);
                            cx.notify();
                            return;
                        }
                        _ => {}
                    }
                }

                // `/` opens search only when inactive
                if !this.search_active && keystroke.key_char.as_deref() == Some("/") {
                    this.toggle_search();
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
                            )
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
                                        if numeric_cols2.is_empty() {
                                            return;
                                        }
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
                                            if numeric_cols3.is_empty() {
                                                return;
                                            }
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

                // Add X==Y warning for Scatter when both columns are the same
                let toolbar = toolbar.when(chart_type == ChartType::Scatter && chart_col == chart_x_col, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(CHART_PEACH))
                            .child("X = Y"),
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

                // Use cached chart data (recomputed on state changes, not every render)
                let chart_data = match &self.chart_data_cache {
                    Some(data) => data.clone(),
                    None => {
                        debug_assert!(false, "chart_data_cache is None when chart is active with data");
                        eprintln!("Bug: chart_data_cache is None when chart should have data");
                        ChartData::Points(Vec::new())
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
                div().flex_1().size_full().child({
                    let entity_for_rows = entity.clone();
                    uniform_list(entity, "rows", filtered_count, {
                        move |this, range, _, _| {
                            this.visible_range = range.clone();
                            range
                                .filter_map(|i| {
                                    let original_idx = *this.filtered_indices.get(i)?;
                                    let selected_col = this.selected_cell.and_then(|(sel_row, col)| {
                                        if sel_row == i { Some(col) } else { None }
                                    });
                                    Some(TableRow {
                                        ix: i,
                                        row_num: original_idx + 1,
                                        cells: this.rows.get(original_idx)?.clone(),
                                        col_widths: this.col_widths.clone(),
                                        row_num_width: this.row_num_width,
                                        min_row_width: viewport_width,
                                        selected_col,
                                        entity: entity_for_rows.clone(),
                                    })
                                })
                                .collect()
                        }
                    })
                    .with_horizontal_sizing_behavior(
                        ListHorizontalSizingBehavior::Unconstrained,
                    )
                    .size_full()
                    .track_scroll(self.scroll_handle.clone())
                }),
            )
            // Status bar
            .child({
                let stats_text: Option<String> = self.selected_cell.and_then(|(_, col)| {
                    let col = col?;
                    if !self.numeric_columns.get(col).copied().unwrap_or(false) {
                        return None;
                    }
                    let stats = compute_column_stats(
                        &self.rows,
                        &self.filtered_indices,
                        col,
                    )?;
                    Some(format!(
                        "Count: {}  Sum: {}  Min: {}  Max: {}  Mean: {}",
                        stats.count,
                        format_stat(stats.sum),
                        format_stat(stats.min),
                        format_stat(stats.max),
                        format_stat(stats.mean),
                    ))
                });

                div()
                    .w_full()
                    .px_2()
                    .py_0p5()
                    .bg(rgb(STATUS_BG))
                    .border_t_1()
                    .border_color(rgb(BORDER_COLOR))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(TEXT_SUBTEXT))
                    .child(
                        div().child(format!("{} / {} rows", filtered_count, total_count)),
                    )
                    .when_some(stats_text, |el, text| {
                        el.child(
                            div().text_color(rgb(TEXT_MAIN)).child(text),
                        )
                    })
            })
    }
}

fn format_stat(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{:.4}", value)
    }
}
