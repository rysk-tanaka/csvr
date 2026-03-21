use std::{ops::Range, rc::Rc};

use regex::RegexBuilder;
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
    ColumnStats, compute_column_stats, compute_column_widths, compute_numeric_columns,
    compute_histogram_bins, downsample, extract_column_values, extract_scatter_pairs,
    filter_columns_by_regex, filter_rows, filter_rows_regex, parse_column_filter,
    row_number_col_width, sort_indices,
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
const STATUS_BG: u32 = HEADER_BG;
const TEXT_ERROR: u32 = 0xf38ba8; // Catppuccin Red

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
    visible_col_indices: Rc<Vec<usize>>,
    pinned_col_count: usize,
    h_offset: gpui::Pixels,
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
        let pinned_count = self.pinned_col_count;
        let h_offset = self.h_offset;


        let make_cell = |col_idx: usize, entity: Entity<CsvrApp>| {
            let width = self.col_widths.get(col_idx).copied().unwrap_or(0.0);
            let text: SharedString = self
                .cells
                .get(col_idx)
                .cloned()
                .unwrap_or_default()
                .into();
            let is_cell_selected = matches!(selected_col, Some(Some(c)) if c == col_idx);
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
        };

        // Pinned section: row number + pinned columns, always offset to keep row numbers fixed.
        // Each pinned cell has its own bg to cover scrollable content behind it, which means
        // the parent hover bg does not show through on pinned cells (known GPUI trade-off).
        let pinned_div = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .ml(-h_offset)
            .child({
                let entity = entity.clone();
                div()
                    .id(("row-num", ix))
                    .w(px(self.row_num_width))
                    .flex_shrink_0()
                    .px_1()
                    // bg covers scrollable content that slides behind this cell via ml(-h_offset).
                    // Trade-off: parent hover highlight doesn't reach through this bg (GPUI limitation).
                    .bg(rgb(bg))
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
            .children(self.visible_col_indices.iter().take(pinned_count).map(
                |&col_idx| {
                    let is_selected = matches!(selected_col, Some(Some(c)) if c == col_idx);
                    // bg covers scrollable content behind pinned cells; skip when selected
                    // so CELL_SELECTED_BG (set inside make_cell) is not overwritten.
                    make_cell(col_idx, entity.clone())
                        .when(!is_selected, |el| el.bg(rgb(bg)))
                },
            ));

        // Scrollable section: non-pinned columns (flows naturally with h_offset)
        let scrollable_div = div()
            .flex()
            .flex_row()
            .children(
                self.visible_col_indices
                    .iter()
                    .skip(pinned_count)
                    .map(|&col_idx| make_cell(col_idx, entity.clone())),
            );

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
            .child(pinned_div)
            .child(scrollable_div)
    }
}

pub(crate) struct CsvrApp {
    headers: Vec<SharedString>,
    /// Original headers (not uppercased) for regex matching and column filter parsing
    raw_headers: Vec<String>,
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
    column_stats_cache: Option<ColumnStats>,
    pub(crate) focus_handle: FocusHandle,
    // E3: Column visibility (`*` command)
    col_filter_active: bool,
    col_filter_query: String,
    visible_col_indices: Rc<Vec<usize>>,
    // E2: Column pinning (`f` command)
    pinned_col_count: usize,
    pin_input_active: bool,
    pin_input_query: String,
    // E4: Row regex filter (`&` command)
    row_filter_active: bool,
    row_filter_query: String,
    row_filter_col: Option<usize>,
    row_filter_pattern: String,
    col_filter_error: bool,
    row_filter_error: bool,
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
        let col_count = data.headers.len();
        let numeric_columns = compute_numeric_columns(&data.rows, col_count);
        let raw_headers = data.headers.clone();
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
        let visible_col_indices = Rc::new((0..col_count).collect());
        Self {
            headers,
            raw_headers,
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
            column_stats_cache: None,
            focus_handle: cx.focus_handle(),
            col_filter_active: false,
            col_filter_query: String::new(),
            visible_col_indices,
            pinned_col_count: 0,
            pin_input_active: false,
            pin_input_query: String::new(),
            row_filter_active: false,
            row_filter_query: String::new(),
            row_filter_col: None,
            row_filter_pattern: String::new(),
            col_filter_error: false,
            row_filter_error: false,
        }
    }

    fn recompute_filtered_indices(&mut self) {
        self.filtered_indices = filter_rows(&self.rows, &self.search_query);
        // Apply `&` regex row filter
        if !self.row_filter_pattern.is_empty() {
            match filter_rows_regex(
                &self.rows,
                &self.filtered_indices,
                &self.row_filter_pattern,
                self.row_filter_col,
            ) {
                Ok(filtered) => {
                    self.row_filter_error = false;
                    self.filtered_indices = filtered;
                }
                Err(_) => {
                    self.row_filter_error = true;
                    // filtered_indices keeps the `/` search-only result; `&` filter is not applied.
                }
            }
        } else {
            self.row_filter_error = false;
        }
        if let Some((col, direction)) = self.sort_state {
            let use_numeric = self.numeric_columns.get(col).copied().unwrap_or(false);
            self.filtered_indices =
                sort_indices(&self.rows, &self.filtered_indices, col, use_numeric, direction);
        }
        self.selected_cell = None;
        self.column_stats_cache = None;
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
        self.search_active = !self.search_active;
    }

    fn close_search(&mut self) {
        self.search_active = false;
        // Keep current filter (consistent with `*` and `&` Escape behavior).
        // User can clear by reopening `/` and deleting the query.
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

    fn recompute_column_stats(&mut self) {
        self.column_stats_cache = self.selected_cell
            .and_then(|(_, col)| col)
            .filter(|&col| self.numeric_columns.get(col).copied().unwrap_or(false))
            .and_then(|col| compute_column_stats(&self.rows, &self.filtered_indices, col));
    }

    fn select_cell(&mut self, filtered_idx: usize, col: Option<usize>) {
        if self.filtered_indices.is_empty() {
            self.clear_selection();
            return;
        }
        let clamped = filtered_idx.min(self.filtered_indices.len() - 1);
        self.selected_cell = Some((clamped, col));
        self.recompute_column_stats();
    }

    fn clear_selection(&mut self) {
        self.selected_cell = None;
        self.column_stats_cache = None;
    }

    fn move_selection(&mut self, row_delta: isize, col_delta: isize) {
        let row_count = self.filtered_indices.len();
        if row_count == 0 {
            return;
        }
        let vis_cols = &self.visible_col_indices;

        let (row, col) = match self.selected_cell {
            Some((r, c)) => (r, c),
            None => {
                let initial_col = vis_cols.first().copied();
                self.selected_cell = Some((0, initial_col));
                self.recompute_column_stats();
                self.ensure_visible(0);
                return;
            }
        };

        let new_row = (row as isize + row_delta).clamp(0, row_count as isize - 1) as usize;

        let new_col = match col {
            Some(c) if !vis_cols.is_empty() => {
                // Find current position in visible columns, navigate within them.
                // unwrap_or(0): if `c` is not in vis_cols (shouldn't happen — cleared on
                // recompute), fall back to first visible column rather than panicking.
                let cur_pos = vis_cols.iter().position(|&v| v == c).unwrap_or(0);
                let new_pos = (cur_pos as isize + col_delta)
                    .clamp(0, vis_cols.len() as isize - 1) as usize;
                Some(vis_cols[new_pos])
            }
            _ => {
                if col_delta > 0 {
                    vis_cols.first().copied()
                } else {
                    None
                }
            }
        };

        let col_changed = col != new_col;
        self.selected_cell = Some((new_row, new_col));
        if col_changed {
            self.recompute_column_stats();
        }
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
            None => self
                .visible_col_indices
                .iter()
                .filter_map(|&c| row.get(c).cloned())
                .collect::<Vec<_>>()
                .join("\t"),
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

    // --- E3: Column visibility (`*` command) ---

    fn toggle_col_filter(&mut self) {
        self.col_filter_active = !self.col_filter_active;
        if self.col_filter_active {
            // Revalidate error flag without clearing selection or scanning headers.
            // Only check if the pattern compiles.
            if !self.col_filter_query.is_empty() {
                self.col_filter_error = RegexBuilder::new(&self.col_filter_query)
                    .case_insensitive(true)
                    .build()
                    .is_err();
            } else {
                self.col_filter_error = false;
            }
        }
    }

    fn set_col_filter_query(&mut self, pattern: String) {
        self.col_filter_query = pattern;
        self.recompute_visible_columns();
    }

    fn close_col_filter(&mut self) {
        self.col_filter_active = false;
        self.col_filter_error = false;
        // Keep current filter (consistent Escape behavior across all modes).
        // User can clear by reopening `*` and deleting the pattern.
    }

    fn recompute_visible_columns(&mut self) {
        match filter_columns_by_regex(&self.raw_headers, &self.col_filter_query) {
            Ok(indices) => {
                self.col_filter_error = false;
                self.visible_col_indices = Rc::new(indices);
                // Clear selection since column positions may have changed
                self.clear_selection();
            }
            Err(_) => {
                self.col_filter_error = true;
                // Fall back to all columns so user can see full data while fixing the pattern.
                // Do NOT clear selection: fallback restores all columns, so existing
                // selection remains valid.
                self.visible_col_indices = Rc::new((0..self.raw_headers.len()).collect());
            }
        }
    }

    // --- E2: Column pinning (`f` command) ---

    fn toggle_pin_input(&mut self) {
        self.pin_input_active = !self.pin_input_active;
        self.pin_input_query.clear();
    }

    fn confirm_pin_input(&mut self) {
        let max = self.visible_col_indices.len();
        self.pinned_col_count = if self.pin_input_query.is_empty() {
            // Empty input resets pinning to 0
            0
        } else {
            // Input is restricted to ASCII digits by the key handler, so parse only
            // fails on overflow (e.g. 99999999999999999999). Clamp to max in that case.
            self.pin_input_query
                .parse::<usize>()
                .unwrap_or(max)
                .min(max)
        };
        self.pin_input_active = false;
        self.pin_input_query.clear();
    }

    fn cancel_pin_input(&mut self) {
        self.pin_input_active = false;
        self.pin_input_query.clear();
    }

    // --- E4: Row regex filter (`&` command) ---

    fn toggle_row_filter(&mut self) {
        self.row_filter_active = !self.row_filter_active;
        if self.row_filter_active {
            // Revalidate error flag without clearing selection or re-scanning rows.
            // Only check if the pattern compiles — no need to run it against all rows.
            if !self.row_filter_pattern.is_empty() {
                self.row_filter_error = RegexBuilder::new(&self.row_filter_pattern)
                    .case_insensitive(true)
                    .build()
                    .is_err();
            } else {
                self.row_filter_error = false;
            }
        }
    }

    fn set_row_filter_query(&mut self, query: String) {
        let (col, pattern) = parse_column_filter(&query, &self.raw_headers);
        self.row_filter_query = query;
        self.row_filter_col = col;
        self.row_filter_pattern = pattern;
        self.recompute_filtered_indices();
        self.scroll_handle
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
    }

    fn close_row_filter(&mut self) {
        self.row_filter_active = false;
        self.row_filter_error = false;
        // Keep current filter (consistent Escape behavior across all modes).
        // User can clear by reopening `&` and deleting the query.
    }

    /// Check if any input mode is active (used to gate key triggers)
    fn any_input_active(&self) -> bool {
        self.search_active || self.col_filter_active || self.pin_input_active || self.row_filter_active
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
                if !this.any_input_active() || this.search_active {
                    this.toggle_search();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &DismissSearch, _window, cx| {
                if this.col_filter_active {
                    this.close_col_filter();
                    cx.notify();
                } else if this.pin_input_active {
                    this.cancel_pin_input();
                    cx.notify();
                } else if this.row_filter_active {
                    this.close_row_filter();
                    cx.notify();
                } else if this.search_active {
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

                // Arrow key navigation (works when no text input mode is active)
                if !keystroke.modifiers.platform && !keystroke.modifiers.control
                    && !this.any_input_active()
                {
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

                // --- E3: `*` column filter input mode ---
                if this.col_filter_active {
                    if keystroke.modifiers.platform || keystroke.modifiers.control {
                        return;
                    }
                    if keystroke.key == "enter" {
                        this.close_col_filter();
                        cx.notify();
                    } else if keystroke.key == "backspace" {
                        let mut q = this.col_filter_query.clone();
                        q.pop();
                        this.set_col_filter_query(q);
                        cx.notify();
                    } else if let Some(ch) = &keystroke.key_char {
                        let mut q = this.col_filter_query.clone();
                        q.push_str(ch);
                        this.set_col_filter_query(q);
                        cx.notify();
                    }
                    return;
                }

                // --- E2: `f` pin input mode ---
                if this.pin_input_active {
                    if keystroke.modifiers.platform || keystroke.modifiers.control {
                        return;
                    }
                    if keystroke.key == "enter" {
                        this.confirm_pin_input();
                        cx.notify();
                    } else if keystroke.key == "backspace" {
                        this.pin_input_query.pop();
                        cx.notify();
                    } else if let Some(ch) = &keystroke.key_char
                        && ch.chars().all(|c| c.is_ascii_digit())
                    {
                        this.pin_input_query.push_str(ch);
                        cx.notify();
                    }
                    return;
                }

                // --- E4: `&` row filter input mode ---
                if this.row_filter_active {
                    if keystroke.modifiers.platform || keystroke.modifiers.control {
                        return;
                    }
                    if keystroke.key == "enter" {
                        this.close_row_filter();
                        cx.notify();
                    } else if keystroke.key == "backspace" {
                        let mut q = this.row_filter_query.clone();
                        q.pop();
                        this.set_row_filter_query(q);
                        cx.notify();
                    } else if let Some(ch) = &keystroke.key_char {
                        let mut q = this.row_filter_query.clone();
                        q.push_str(ch);
                        this.set_row_filter_query(q);
                        cx.notify();
                    }
                    return;
                }

                // --- Existing: `/` search input mode ---
                if this.search_active {
                    if keystroke.modifiers.platform || keystroke.modifiers.control {
                        return;
                    }
                    if keystroke.key == "enter" {
                        this.close_search();
                        cx.notify();
                    } else if keystroke.key == "backspace" {
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
                    return;
                }

                // --- Normal mode: trigger keys for input modes ---
                if !keystroke.modifiers.platform && !keystroke.modifiers.control {
                    match keystroke.key_char.as_deref() {
                        Some("/") => {
                            this.toggle_search();
                            cx.notify();
                        }
                        Some("*") => {
                            this.toggle_col_filter();
                            cx.notify();
                        }
                        Some("&") => {
                            this.toggle_row_filter();
                            cx.notify();
                        }
                        Some("f") => {
                            this.toggle_pin_input();
                            cx.notify();
                        }
                        _ => {}
                    }
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
                    // Inner row: pinned section + scrollable section
                    .child({
                        let pinned_count = self.pinned_col_count.min(self.visible_col_indices.len());


                        let make_header_cell = |col_idx: usize, entity: Entity<CsvrApp>| {
                            let width = self.col_widths.get(col_idx).copied().unwrap_or(0.0);
                            let label = self.headers.get(col_idx).cloned().unwrap_or_else(|| "".into());
                            let indicator: SharedString = match self.sort_state {
                                Some((c, SortDirection::Ascending)) if c == col_idx => "▲".into(),
                                Some((c, SortDirection::Descending)) if c == col_idx => "▼".into(),
                                _ => "".into(),
                            };
                            let has_indicator = !indicator.is_empty();
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
                                        .child(label),
                                )
                                .when(has_indicator, |el| {
                                    el.child(div().flex_shrink_0().ml_0p5().child(indicator))
                                })
                        };

                        // Pinned section: row number + pinned header columns, always fixed
                        let pinned_div = div()
                            .flex()
                            .flex_row()
                            .flex_shrink_0()
                            .bg(rgb(HEADER_BG))
                            .ml(-h_offset)
                            .child(
                                div()
                                    .w(px(self.row_num_width))
                                    .flex_shrink_0()
                                    .px_1()
                                    .text_right()
                                    .child("#"),
                            )
                            .children(
                                self.visible_col_indices
                                    .iter()
                                    .take(pinned_count)
                                    .map(|&col_idx| make_header_cell(col_idx, entity.clone())),
                            );

                        // Scrollable section: non-pinned header columns
                        let scrollable_div = div()
                            .flex()
                            .flex_row()
                            .children(
                                self.visible_col_indices
                                    .iter()
                                    .skip(pinned_count)
                                    .map(|&col_idx| make_header_cell(col_idx, entity.clone())),
                            );

                        div()
                            .flex()
                            .flex_row()
                            .ml(h_offset)
                            .child(pinned_div)
                            .child(scrollable_div)
                    }),
            )
            // `*` column filter bar
            .when(self.col_filter_active, |el| {
                let query_display: SharedString = if self.col_filter_query.is_empty() {
                    "Type column regex...".into()
                } else {
                    self.col_filter_query.clone().into()
                };
                let query_color = if self.col_filter_error {
                    TEXT_ERROR
                } else if self.col_filter_query.is_empty() {
                    TEXT_SUBTEXT
                } else {
                    TEXT_MAIN
                };
                let vis_count = self.visible_col_indices.len();
                let total_cols = self.raw_headers.len();
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
                        .child(div().text_color(rgb(TEXT_SUBTEXT)).text_xs().child("*"))
                        .child(div().flex_1().text_color(rgb(query_color)).child(query_display))
                        .child(
                            div()
                                .text_color(rgb(if self.col_filter_error { TEXT_ERROR } else { TEXT_SUBTEXT }))
                                .text_xs()
                                .child(if self.col_filter_error {
                                    "invalid regex".to_string()
                                } else {
                                    format!("{} / {} cols", vis_count, total_cols)
                                }),
                        ),
                )
            })
            // `f` pin input bar
            .when(self.pin_input_active, |el| {
                let display: SharedString = if self.pin_input_query.is_empty() {
                    "Type number of columns to freeze...".into()
                } else {
                    format!("Freeze {} columns", self.pin_input_query).into()
                };
                let display_color = if self.pin_input_query.is_empty() {
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
                        .child(div().text_color(rgb(TEXT_SUBTEXT)).text_xs().child("f"))
                        .child(div().flex_1().text_color(rgb(display_color)).child(display)),
                )
            })
            // `&` row filter bar
            .when(self.row_filter_active, |el| {
                let query_display: SharedString = if self.row_filter_query.is_empty() {
                    "Type row filter regex (or col:regex)...".into()
                } else {
                    self.row_filter_query.clone().into()
                };
                let query_color = if self.row_filter_error {
                    TEXT_ERROR
                } else if self.row_filter_query.is_empty() {
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
                        .child(div().text_color(rgb(TEXT_SUBTEXT)).text_xs().child("&"))
                        .child(div().flex_1().text_color(rgb(query_color)).child(query_display))
                        .child(
                            div()
                                .text_color(rgb(if self.row_filter_error { TEXT_ERROR } else { TEXT_SUBTEXT }))
                                .text_xs()
                                .child(if self.row_filter_error {
                                    "invalid regex".to_string()
                                } else {
                                    format!("{} / {} rows", filtered_count, total_count)
                                }),
                        ),
                )
            })
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
                                        visible_col_indices: this.visible_col_indices.clone(),
                                        pinned_col_count: this.pinned_col_count.min(this.visible_col_indices.len()),
                                        h_offset: this.h_scroll_offset(),
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
                let stats_text: Option<String> = self.column_stats_cache.as_ref().map(|stats| {
                    format!(
                        "Count: {}  Sum: {}  Min: {}  Max: {}  Mean: {}  Median: {}  Stddev: {}",
                        stats.count,
                        format_stat(stats.sum),
                        format_stat(stats.min),
                        format_stat(stats.max),
                        format_stat(stats.mean),
                        format_stat(stats.median),
                        format_stat(stats.stddev),
                    )
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
                    .child({
                        let vis_cols = self.visible_col_indices.len();
                        let total_cols = self.raw_headers.len();
                        let pinned = self.pinned_col_count.min(vis_cols);
                        let mut parts = vec![format!("{} / {} rows", filtered_count, total_count)];
                        if vis_cols < total_cols {
                            parts.push(format!("{} / {} cols", vis_cols, total_cols));
                        }
                        if pinned > 0 {
                            parts.push(format!("f{}", pinned));
                        }
                        div().child(parts.join("  "))
                    })
                    .when_some(stats_text, |el, text| {
                        el.child(
                            div().text_color(rgb(TEXT_MAIN)).child(text),
                        )
                    })
            })
    }
}

fn format_stat(value: f64) -> String {
    // f64 mantissa is 53 bits → integers up to 2^53 (~9.0e15) are exact
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{:.4}", value)
    }
}
