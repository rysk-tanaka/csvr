use std::{collections::HashMap, ops::Range, rc::Rc, sync::Arc, time::Duration};

use futures_lite::future::yield_now;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, Font, FontWeight, Hsla, KeyBinding, KeyDownEvent,
    Keystroke, ListHorizontalSizingBehavior, Pixels, Rems, Render, SharedString, Task, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, UniformListScrollHandle, Window, actions, canvas, div,
    fill, point, prelude::*, px, rgb, uniform_list,
};
use regex::RegexBuilder;

use csvr_core::compute::{
    ColumnStats, MAX_COL_WIDTH, RowFilter, RowPositions, TextInputState, anchored_position,
    caret_scroll_offset, chart_legend, clamp_h_offset, compute_column_stats, compute_column_widths,
    compute_histogram_bins, compute_numeric_columns, copied_status_message, downsample,
    export_json, export_markdown, extract_column_values, extract_scatter_pairs,
    filter_columns_by_regex, h_offset_to_reveal, histogram_axes, measurable_by_pairs,
    pair_needs_shaping, pairs_axes, parse_column_filter, points_axes, reorder_indices,
    row_number_col_width, sort_indices,
};
use csvr_core::data::{
    CHART_TYPES, ChartAxes, ChartData, ChartType, CsvData, SortDirection, Table,
};

use crate::chart::{CHART_BLUE, CHART_CANVAS_HEIGHT, CHART_GREEN, CHART_PEACH, draw_chart};

actions!(
    csvr,
    [
        ToggleSearch,
        DismissSearch,
        ToggleChart,
        ToggleMetadata,
        CopySelection,
        ExportJson,
        ExportMarkdown,
        Quit
    ]
);

/// Modifier for the table view's shortcuts. The browser build binds both, because its `cmd`
/// is the Meta key on every OS.
#[derive(Clone, Copy)]
pub(crate) enum ShortcutModifier {
    Cmd,
    #[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
    Ctrl,
}

/// Bindings without a modifier come once, however many modifiers are given.
pub(crate) fn key_bindings(modifiers: &[ShortcutModifier]) -> Vec<KeyBinding> {
    // None of the bindings apply while the IME is composing. On the web, keys reach the app
    // before the IME, so Escape or `cmd-f` would otherwise close the bar under the composition.
    // The view sets `ime_editing` in its key context (see `render`), and GPUI redraws a changed
    // view before it dispatches the next key, so the context is current.
    let context = Some("CsvrApp && !ime_editing");
    let mut bindings = vec![KeyBinding::new("escape", DismissSearch, context)];
    for modifier in modifiers {
        // `KeyBinding::new` takes `&'static str`, so the keystrokes are spelled out per modifier.
        let [find, chart, copy, json, markdown] = match modifier {
            ShortcutModifier::Cmd => ["cmd-f", "cmd-g", "cmd-c", "cmd-shift-j", "cmd-shift-m"],
            ShortcutModifier::Ctrl => {
                ["ctrl-f", "ctrl-g", "ctrl-c", "ctrl-shift-j", "ctrl-shift-m"]
            }
        };
        bindings.extend([
            KeyBinding::new(find, ToggleSearch, context),
            KeyBinding::new(chart, ToggleChart, context),
            KeyBinding::new(copy, CopySelection, context),
            KeyBinding::new(json, ExportJson, context),
            KeyBinding::new(markdown, ExportMarkdown, context),
        ]);
    }
    bindings
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
const SURFACE1: u32 = 0x45475a;
const ROW_HOVER_BG: u32 = 0x27273a; // Between Base and Surface0 — hover
const ROW_SELECTED_BG: u32 = 0x313244; // Surface0 — selected row
const CELL_SELECTED_BG: u32 = 0x45475a; // Surface1 — selected cell
const STATUS_BG: u32 = HEADER_BG;
const TEXT_ERROR: u32 = 0xf38ba8; // Catppuccin Red
const TEXT_WARNING: u32 = 0xf9e2af; // Catppuccin Yellow
const STATUS_MESSAGE_DURATION: Duration = Duration::from_secs(3);
/// Separates clipped columns from the row numbers, where a cut-off value can read as part of the
/// number. Counted in the content width so the last column still reaches the right edge.
const PINNED_BORDER_WIDTH: f32 = 1.0;
/// Width of the caret in the `/`, `*` and `&` bars.
const CARET_WIDTH: f32 = 1.0;
/// The table's font. The column widths are measured with it, so the sizes below are both drawn
/// and measured: `text_sm` for the cells and the row numbers, bold `text_xs` for the header.
const TABLE_FONT_FAMILY: &str = ".SystemUIFont";
const CELL_TEXT_SIZE: Rems = Rems(0.875);
const HEADER_TEXT_SIZE: Rems = Rems(0.75);
const HEADER_FONT_WEIGHT: FontWeight = FontWeight::BOLD;
/// The gap before the sort indicator (`ml_0p5`).
const SORT_INDICATOR_GAP: Rems = Rems(0.125);
/// Characters of a long line shaped first: a line that long is usually wider than any column
/// can be, and the prefix settles that without shaping (and caching the layout of) the rest.
const SHAPED_PREFIX_CHARS: usize = 256;
/// Rows a filter task checks between yields. Small enough that a replaced task stops soon and a
/// single-threaded browser build (gpui_web, `src/web.rs`), which runs background tasks on the UI
/// thread, still handles input; large enough that yielding costs little.
const FILTER_CHUNK_ROWS: usize = 50_000;

/// Decided when a task starts, so the result is shown the way it was requested even if the
/// view changes meanwhile.
enum ScrollTarget {
    /// After a sort, since the old position means nothing in the new order.
    Top,
    /// Where each row went, to anchor the view as it is when the result arrives.
    Keep(RowPositions),
}

/// What a filter task orders. Only the source it needs is moved into the task.
enum RowSource {
    /// A query changed, so every row is checked again. The queries are kept as text so that
    /// the `/` regex is built in the task rather than on the UI thread at every keystroke. The
    /// `&` pattern is still compiled once on the UI thread by `is_valid_pattern`, to show its
    /// error without waiting for the task.
    Filter {
        query: String,
        pattern: String,
        target_col: Option<usize>,
    },
    /// Only the sort changed, so the rows already shown are reordered.
    Reorder(Arc<Vec<usize>>),
}

struct FilterResult {
    indices: Vec<usize>,
    /// Whether the rows were filtered again, rather than only the shown rows reordered.
    is_refiltered: bool,
    scroll: ScrollTarget,
}

#[derive(IntoElement)]
struct TableRow {
    ix: usize,
    rows: Arc<Table>,
    /// Index of this row in `rows`; the row number shown is one more.
    row: usize,
    col_widths: Rc<Vec<f32>>,
    row_num_width: f32,
    row_width: gpui::Pixels,
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

        let row = self.rows.row(self.row);
        let make_cell = |col_idx: usize, entity: Entity<CsvrApp>| {
            let width = self.col_widths.get(col_idx).copied().unwrap_or(0.0);
            let cell = row.and_then(|row| row.get(col_idx)).unwrap_or("");
            let text = SharedString::from(cell.to_string());
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

        // Pinned section: row number + pinned columns, offset to cancel the list's horizontal shift.
        let pinned_div = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .ml(-h_offset)
            .border_r(px(PINNED_BORDER_WIDTH))
            .border_color(rgb(BORDER_COLOR))
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
                    .child((self.row + 1).to_string())
            })
            .children(
                self.visible_col_indices
                    .iter()
                    .take(pinned_count)
                    .map(|&col_idx| make_cell(col_idx, entity.clone())),
            );

        // Scrollable section: clipped to the area right of the pinned section, so its cells never
        // overlap the pinned section on screen or under the mouse.
        let scrollable_div = div()
            .flex_1()
            .flex()
            .flex_row()
            .overflow_hidden()
            .py_0p5()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .ml(h_offset)
                    .children(
                        self.visible_col_indices
                            .iter()
                            .skip(pinned_count)
                            .map(|&col_idx| make_cell(col_idx, entity.clone())),
                    ),
            );

        div()
            .id(("row", ix))
            .flex()
            .flex_row()
            .w(self.row_width)
            .border_b_1()
            .border_color(rgb(BORDER_COLOR))
            .bg(rgb(bg))
            .when(!is_row_selected, |el| {
                el.hover(|style| style.bg(rgb(ROW_HOVER_BG)))
            })
            .child(pinned_div)
            .child(scrollable_div)
    }
}

pub(crate) struct CsvrApp {
    /// Metadata rows extracted before the promoted header (key-value pairs, section labels, etc.)
    metadata: Vec<Vec<String>>,
    /// Warnings from loading the input, kept visible because they describe the data on screen
    load_warning: Option<SharedString>,
    status_message: Option<SharedString>,
    /// Replacing this cancels the previous timer, so a newer message keeps its full duration
    status_message_task: Option<Task<()>>,
    headers: Vec<SharedString>,
    /// Original headers (not uppercased) for regex matching and column filter parsing
    raw_headers: Vec<String>,
    /// Shared with filter tasks running off the UI thread.
    rows: Arc<Table>,
    col_widths: Rc<Vec<f32>>,
    numeric_columns: Vec<bool>,
    row_num_width: f32,
    scroll_handle: UniformListScrollHandle,
    visible_range: Range<usize>,
    /// The rows and top position from before the filter results became empty. GPUI resets the
    /// offset of an empty list, so this is what lets the view return when the results come back.
    rows_before_empty: Option<(Arc<Vec<usize>>, usize)>,
    search_active: bool,
    search_query: String,
    /// Replaced only when a filter task's result is applied. Held in an `Arc` so a task can
    /// take the order it started from without copying it.
    filtered_indices: Arc<Vec<usize>>,
    /// Replacing this drops the running filter task, so only the latest query is applied.
    filter_task: Option<Task<()>>,
    /// Set by a sort. The result then scrolls to the top instead of keeping the position, even
    /// if typing replaces the task before it finishes.
    pending_scroll_to_top: bool,
    /// Set when a `/` or `&` query changes, until a filtered result is shown. While it is clear,
    /// the shown rows match the queries, so a sort only needs to reorder them.
    needs_refilter: bool,
    sort_state: Option<(usize, SortDirection)>,
    chart_active: bool,
    metadata_visible: bool,
    chart_type: ChartType,
    chart_col: usize,
    chart_x_col: usize,
    chart_data_cache: Option<(ChartData, ChartAxes)>,
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
    /// What the open `/`, `*` or `&` bar is editing, including the IME's composing text. The
    /// queries above keep the last committed text, so the filters never see composing text.
    text_input: Option<TextInputState>,
}

impl Focusable for CsvrApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CsvrApp {
    /// Measures the columns again, for when the fonts change: the browser build adds the Japanese
    /// font once text needs it, and text measured before that has the width of the missing glyph.
    #[cfg(target_family = "wasm")]
    pub(crate) fn remeasure_columns(&mut self, window: &Window) {
        let (col_widths, row_num_width) = measure_columns(window, &self.rows, &self.headers);
        self.col_widths = Rc::new(col_widths);
        self.row_num_width = row_num_width;
    }

    // HACK: GPUI has no public API for horizontal scroll offset on UniformListScrollHandle.
    // Access internal fields directly. Replace when a public API becomes available.
    fn h_scroll_offset(&self) -> gpui::Pixels {
        self.scroll_handle.0.borrow().base_handle.offset().x
    }

    // HACK: Same as h_scroll_offset. Keep the vertical offset, which scroll_to_item updates
    // separately during prepaint.
    fn set_h_scroll_offset(&self, x: gpui::Pixels) {
        let state = self.scroll_handle.0.borrow();
        let y = state.base_handle.offset().y;
        state.base_handle.set_offset(gpui::point(x, y));
    }

    pub(crate) fn new(data: CsvData, window: &Window, cx: &mut Context<Self>) -> Self {
        let col_count = data.headers.len();
        let numeric_columns = compute_numeric_columns(&data.rows, col_count);
        let raw_headers = data.headers.clone();
        let headers: Vec<SharedString> = data
            .headers
            .iter()
            .map(|h| SharedString::from(h.to_uppercase()))
            .collect();
        let total_rows = data.rows.row_count();
        let rows = Arc::new(data.rows);
        let (col_widths, row_num_width) = measure_columns(window, &rows, &headers);
        // Find first two numeric columns for chart defaults
        let first_numeric = numeric_columns.iter().position(|&b| b).unwrap_or(0);
        let second_numeric = numeric_columns
            .iter()
            .skip(first_numeric + 1)
            .position(|&b| b)
            .map(|i| i + first_numeric + 1)
            .unwrap_or(first_numeric);
        let visible_col_indices = Rc::new((0..col_count).collect());
        let has_metadata = !data.metadata.is_empty();
        let load_warning =
            (!data.warnings.is_empty()).then(|| SharedString::from(data.warnings.join("  ")));
        Self {
            metadata: data.metadata,
            load_warning,
            status_message: None,
            status_message_task: None,
            headers,
            raw_headers,
            rows,
            col_widths: Rc::new(col_widths),
            numeric_columns,
            row_num_width,
            scroll_handle: UniformListScrollHandle::new(),
            visible_range: 0..0,
            rows_before_empty: None,
            search_active: false,
            search_query: String::new(),
            filtered_indices: Arc::new((0..total_rows).collect()),
            filter_task: None,
            pending_scroll_to_top: false,
            needs_refilter: false,
            sort_state: None,
            chart_active: false,
            metadata_visible: has_metadata,
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
            text_input: None,
        }
    }

    /// Start filtering and sorting off the UI thread. The view keeps showing the previous
    /// result until `apply_filter_result` replaces it, so selection, copy and export stay
    /// consistent with what is on screen.
    fn recompute_filtered_indices(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows.clone();
        // A sort alone keeps the same rows, so the shown rows are reordered instead of filtered
        // again. They are the full result for the current queries unless a query changed.
        let source = if self.needs_refilter {
            RowSource::Filter {
                query: self.search_query.clone(),
                pattern: self.row_filter_pattern.clone(),
                target_col: self.row_filter_col,
            }
        } else {
            RowSource::Reorder(self.filtered_indices.clone())
        };
        let sort = self.sort_state.map(|(col, direction)| {
            let use_numeric = self.numeric_columns.get(col).copied().unwrap_or(false);
            (col, use_numeric, direction)
        });
        let scrolls_to_top = self.pending_scroll_to_top;
        self.filter_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let is_refiltered = matches!(source, RowSource::Filter { .. });
                    let mut indices = match source {
                        RowSource::Filter {
                            query,
                            pattern,
                            target_col,
                        } => {
                            let filter = RowFilter::new(&query, &pattern, target_col);
                            let mut indices = Vec::new();
                            for start in (0..rows.row_count()).step_by(FILTER_CHUNK_ROWS) {
                                let chunk = start..start + FILTER_CHUNK_ROWS;
                                filter.filter_range(&rows, chunk, &mut indices);
                                // Lets a replaced task stop here, and lets the single-threaded
                                // browser build handle input in between.
                                yield_now().await;
                            }
                            indices
                        }
                        RowSource::Reorder(shown) => reorder_indices(&rows, &shown, sort),
                    };
                    if is_refiltered && let Some((col, use_numeric, direction)) = sort {
                        indices = sort_indices(&rows, &indices, col, use_numeric, direction);
                    }
                    yield_now().await;
                    let scroll = if scrolls_to_top {
                        ScrollTarget::Top
                    } else {
                        ScrollTarget::Keep(RowPositions::new(&indices, rows.row_count()))
                    };
                    FilterResult {
                        indices,
                        is_refiltered,
                        scroll,
                    }
                })
                .await;
            this.update(cx, |this, cx| this.apply_filter_result(result, cx))
                .ok();
        }));
    }

    /// The order and top position a result is anchored to. While the results are empty, these
    /// come from before they became empty, since GPUI resets the offset of an empty list.
    fn anchor_view(&self) -> (Arc<Vec<usize>>, usize) {
        if self.filtered_indices.is_empty() {
            self.rows_before_empty.clone().unwrap_or_default()
        } else {
            (self.filtered_indices.clone(), self.visible_range.start)
        }
    }

    /// Show a finished filter result, keeping the view on the same rows where possible
    /// (or scrolling to the top after a sort). Anchors to the view as it is now, which may have
    /// scrolled or changed its selection while the task ran.
    fn apply_filter_result(&mut self, result: FilterResult, cx: &mut Context<Self>) {
        let FilterResult {
            indices,
            is_refiltered,
            scroll,
        } = result;
        // This runs inside the task itself, so detach rather than drop it; either way it ends here.
        if let Some(task) = self.filter_task.take() {
            task.detach();
        }
        let (previous, top) = self.anchor_view();
        let selected = self
            .selected_cell
            .and_then(|(row, col)| Some((*previous.get(row)?, col)));

        self.pending_scroll_to_top = false;
        if is_refiltered {
            self.needs_refilter = false;
        }
        self.rows_before_empty = None;
        self.filtered_indices = Arc::new(indices);
        self.selected_cell = None;
        self.column_stats_cache = None;

        match scroll {
            ScrollTarget::Top => {
                self.scroll_handle
                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
                // The list only updates this when it renders, and another key can arrive before that.
                self.visible_range = 0..self.visible_range.len();
            }
            ScrollTarget::Keep(positions) => {
                if let Some((row, col)) = selected
                    && let Some(pos) = positions.get(row)
                {
                    self.selected_cell = Some((pos, col));
                    self.recompute_column_stats();
                }
                if let Some(pos) = anchored_position(&previous, &positions, top) {
                    self.scroll_handle
                        .scroll_to_item_strict(pos, gpui::ScrollStrategy::Top);
                    self.visible_range = pos..pos + self.visible_range.len();
                }
                if self.filtered_indices.is_empty() {
                    self.rows_before_empty = Some((previous, top));
                }
            }
        }
        if self.chart_active {
            self.recompute_chart_data();
        }
        cx.notify();
    }

    /// Whether a filter task is still running, so the shown rows may not match the queries yet.
    fn is_filtering(&self) -> bool {
        self.filter_task.is_some()
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.needs_refilter = true;
        self.recompute_filtered_indices(cx);
    }

    fn toggle_sort(&mut self, col: usize, cx: &mut Context<Self>) {
        self.sort_state = match self.sort_state {
            Some((c, SortDirection::Ascending)) if c == col => {
                Some((col, SortDirection::Descending))
            }
            Some((c, SortDirection::Descending)) if c == col => None,
            _ => Some((col, SortDirection::Ascending)),
        };
        // The saved order no longer matches the sorted rows.
        self.rows_before_empty = None;
        self.pending_scroll_to_top = true;
        self.recompute_filtered_indices(cx);
    }

    fn toggle_search(&mut self) {
        self.search_active = !self.search_active;
        self.text_input = self
            .search_active
            .then(|| TextInputState::new(self.search_query.clone()));
    }

    fn close_search(&mut self) {
        self.search_active = false;
        self.text_input = None;
        // Keep current filter (consistent with `*` and `&` Escape behavior).
        // User can clear by reopening `/` and deleting the query.
    }

    fn toggle_metadata(&mut self) {
        if !self.metadata.is_empty() {
            self.metadata_visible = !self.metadata_visible;
        }
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
                let values: Vec<f64> = sampled.into_iter().map(|(_, v)| v).collect();
                let axes = points_axes(&values, true);
                (ChartData::Points(values), axes)
            }
            ChartType::Line => {
                let sampled = downsample(&y_values, 500);
                let values: Vec<f64> = sampled.into_iter().map(|(_, v)| v).collect();
                let axes = points_axes(&values, false);
                (ChartData::Points(values), axes)
            }
            ChartType::Histogram => {
                let vals: Vec<f64> = y_values.iter().map(|(_, v)| *v).collect();
                let bins = compute_histogram_bins(&vals, 30);
                let axes = histogram_axes(&vals, &bins);
                (ChartData::Bins(bins), axes)
            }
            ChartType::Scatter => {
                let pairs = extract_scatter_pairs(
                    &self.rows,
                    &self.filtered_indices,
                    self.chart_x_col,
                    self.chart_col,
                );
                let limited = downsample(&pairs, 500);
                let axes = pairs_axes(&limited);
                (ChartData::Pairs(limited), axes)
            }
        });
    }

    fn recompute_column_stats(&mut self) {
        self.column_stats_cache = self
            .selected_cell
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

    fn move_selection(&mut self, row_delta: isize, col_delta: isize, viewport_width: f32) {
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
                self.ensure_col_visible(initial_col, viewport_width);
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
                let new_pos =
                    (cur_pos as isize + col_delta).clamp(0, vis_cols.len() as isize - 1) as usize;
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
        self.ensure_col_visible(new_col, viewport_width);
    }

    fn ensure_visible(&self, filtered_idx: usize) {
        if filtered_idx < self.visible_range.start || filtered_idx >= self.visible_range.end {
            self.scroll_handle
                .scroll_to_item(filtered_idx, gpui::ScrollStrategy::Center);
        }
    }

    fn ensure_col_visible(&self, col: Option<usize>, viewport_width: f32) {
        let Some(col) = col else {
            return;
        };
        let pinned_count = self.pinned_col_count.min(self.visible_col_indices.len());
        let (pinned, scrollable) = self.visible_col_indices.split_at(pinned_count);
        // Pinned columns are always on screen.
        let Some(pos) = scrollable.iter().position(|&c| c == col) else {
            return;
        };
        let total_width = |cols: &[usize]| {
            cols.iter()
                .filter_map(|&c| self.col_widths.get(c))
                .sum::<f32>()
        };
        let pinned_width = self.row_num_width + PINNED_BORDER_WIDTH + total_width(pinned);
        let offset = h_offset_to_reveal(
            self.h_scroll_offset().as_f32(),
            total_width(&scrollable[..pos]),
            self.col_widths.get(col).copied().unwrap_or(0.0),
            viewport_width - pinned_width,
        );
        self.set_h_scroll_offset(px(offset));
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        let Some((filtered_idx, col)) = self.selected_cell else {
            return;
        };
        let Some(&original_idx) = self.filtered_indices.get(filtered_idx) else {
            return;
        };
        let Some(row) = self.rows.row(original_idx) else {
            return;
        };

        let text = match col {
            Some(c) => row.get(c).unwrap_or("").to_string(),
            None => self
                .visible_col_indices
                .iter()
                .filter_map(|&c| row.get(c))
                .collect::<Vec<_>>()
                .join("\t"),
        };

        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Exports would otherwise copy the rows from before the latest query, which the user is
    /// likely to take for the filtered result.
    fn is_ready_to_export(&mut self, cx: &mut Context<Self>) -> bool {
        if self.is_filtering() {
            self.show_status_message("Still filtering. Export again when it finishes", cx);
            return false;
        }
        true
    }

    fn export_json(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready_to_export(cx) {
            return;
        }
        let text = export_json(
            &self.raw_headers,
            &self.rows,
            &self.filtered_indices,
            &self.visible_col_indices,
        );
        self.finish_export(text, "JSON", cx);
    }

    fn export_markdown(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready_to_export(cx) {
            return;
        }
        let text = export_markdown(
            &self.raw_headers,
            &self.rows,
            &self.filtered_indices,
            &self.visible_col_indices,
        );
        self.finish_export(text, "Markdown", cx);
    }

    fn finish_export(&mut self, text: String, format_name: &str, cx: &mut Context<Self>) {
        let row_count = self.filtered_indices.len();
        // Exports without data still produce text ("[]" or a bare header), which would replace
        // the clipboard contents with nothing useful.
        if row_count == 0 || self.visible_col_indices.is_empty() {
            self.show_status_message("No data to export", cx);
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.show_status_message(copied_status_message(format_name, row_count), cx);
    }

    fn show_status_message(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status_message = Some(text.into());
        self.status_message_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(STATUS_MESSAGE_DURATION)
                .await;
            this.update(cx, |this, cx| {
                this.status_message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
        self.text_input = self
            .col_filter_active
            .then(|| TextInputState::new(self.col_filter_query.clone()));
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
        self.text_input = None;
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
        self.text_input = self
            .row_filter_active
            .then(|| TextInputState::new(self.row_filter_query.clone()));
        if self.row_filter_active {
            // Revalidate error flag without clearing selection or re-scanning rows.
            // Only check if the pattern compiles — no need to run it against all rows.
            self.row_filter_error = !RowFilter::is_valid_pattern(&self.row_filter_pattern);
        }
    }

    fn set_row_filter_query(&mut self, query: String, cx: &mut Context<Self>) {
        let (col, pattern) = parse_column_filter(&query, &self.raw_headers);
        self.row_filter_query = query;
        self.row_filter_col = col;
        // Checked here rather than with the result, so the error shows as soon as it is typed
        // and a result landing after the bar closes cannot set it again.
        self.row_filter_error = !RowFilter::is_valid_pattern(&pattern);
        self.row_filter_pattern = pattern;
        self.needs_refilter = true;
        self.recompute_filtered_indices(cx);
    }

    fn close_row_filter(&mut self) {
        self.row_filter_active = false;
        self.text_input = None;
        self.row_filter_error = false;
        // Keep current filter (consistent Escape behavior across all modes).
        // User can clear by reopening `&` and deleting the query.
    }

    /// Check if any input mode is active (used to gate key triggers)
    fn any_input_active(&self) -> bool {
        self.search_active
            || self.col_filter_active
            || self.pin_input_active
            || self.row_filter_active
    }

    // --- Text input for the `/`, `*` and `&` bars ---

    /// Whether the IME is in the middle of an edit, so keys belong to it and the query waits.
    fn is_ime_editing(&self) -> bool {
        self.text_input.as_ref().is_some_and(|input| {
            // The browser keeps composing after the composing text becomes empty, until
            // compositionend, and keys still reach the app first in the meantime. macOS can end
            // a composition with an empty marked text and no unmark, so the flag would stay set
            // there; it routes keys to the IME first anyway while text is marked.
            input.is_composing() || (cfg!(target_family = "wasm") && input.is_composition_open())
        })
    }

    /// Puts the edited text into the open bar's query once the IME is done with it.
    fn commit_text_input(&mut self, cx: &mut Context<Self>) {
        if self.is_ime_editing() {
            return;
        }
        let Some(text) = self
            .text_input
            .as_ref()
            .map(|input| input.text().to_string())
        else {
            return;
        };
        // Same order as the exclusive modes. Unchanged queries are skipped so that edits that
        // only move the composition do not refilter.
        if self.col_filter_active {
            if text != self.col_filter_query {
                self.set_col_filter_query(text);
            }
        } else if self.row_filter_active {
            if text != self.row_filter_query {
                self.set_row_filter_query(text, cx);
            }
        } else if self.search_active && text != self.search_query {
            self.set_search_query(text, cx);
        }
    }

    fn after_text_input_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_text_input(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn close_text_input_bar(&mut self) {
        if self.col_filter_active {
            self.close_col_filter();
        } else if self.row_filter_active {
            self.close_row_filter();
        } else if self.search_active {
            self.close_search();
        }
    }

    /// Keys for the `/`, `*` and `&` bars. Typed text is left alone: GPUI hands it to the input
    /// handler afterwards, so adding it here too would insert it twice. Handled keys stop
    /// propagation, or the browser would also apply them to its hidden textarea.
    fn handle_text_input_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // On the web, keys reach the app before the IME while composing.
        if self.is_ime_editing() {
            return;
        }
        // The bars do not move the caret with these keys, and macOS GPUI would not either.
        // Stopping them keeps the browser from moving the caret of its hidden textarea.
        let is_navigation_key = matches!(
            keystroke.key.as_str(),
            "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown"
        );
        if is_navigation_key {
            cx.stop_propagation();
            return;
        }
        let modifiers = &keystroke.modifiers;
        // Handled here because the browser's hidden textarea only holds the text around the
        // caret, so its select-all misses the rest of a long query.
        // macOS uses ctrl-a for the start of the line, so the native build takes only cmd-a. The
        // browser build also takes ctrl-a, the select-all of Windows and Linux.
        let has_select_all_modifier = if cfg!(target_family = "wasm") {
            modifiers.platform != modifiers.control
        } else {
            modifiers.platform && !modifiers.control
        };
        let is_select_all = keystroke.key == "a"
            && has_select_all_modifier
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.function;
        if is_select_all {
            if let Some(input) = &mut self.text_input {
                input.select_all();
            }
            window.notify_text_selection_changed(&self.focus_handle);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if modifiers.platform || modifiers.control {
            return;
        }
        match keystroke.key.as_str() {
            "enter" => {
                self.close_text_input_bar();
                cx.notify();
                cx.stop_propagation();
            }
            "backspace" => {
                if let Some(input) = &mut self.text_input {
                    input.delete_backward();
                }
                self.after_text_input_edit(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

impl EntityInputHandler for CsvrApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (text, range) = self.text_input.as_ref()?.text_for_range_utf16(range_utf16);
        actual_range.replace(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.text_input.as_ref()?.selected_range_utf16(),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.text_input.as_ref()?.marked_range_utf16()
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text_input.as_ref()?.len_utf16())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &mut self.text_input else {
            return;
        };
        input.unmark();
        self.after_text_input_edit(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = &mut self.text_input else {
            return;
        };
        input.replace(range_utf16, text);
        self.after_text_input_edit(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = &mut self.text_input else {
            return;
        };
        input.replace_and_mark(range_utf16, new_text, new_selected_range_utf16);
        self.after_text_input_edit(window, cx);
    }

    // Accepted whenever it comes: the browser's hidden textarea has already moved its
    // selection, and the next edit is placed from both, so refusing would put it elsewhere.
    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = &mut self.text_input else {
            return;
        };
        input.set_selected_range_utf16(range_utf16);
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    /// The whole bar: macOS opens the IME candidates below it.
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.text_input.as_ref().map(|_| element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.text_input.is_some()
    }
}

/// The widths of the columns and of the row number column, measured with the fonts the table
/// draws with. Cell lines go through `LineMeasure`; headers are few, so each is shaped whole.
fn measure_columns(window: &Window, rows: &Table, headers: &[SharedString]) -> (Vec<f32>, f32) {
    let rem_size = window.rem_size();
    let cell_size = CELL_TEXT_SIZE.to_pixels(rem_size);
    let header_font = Font {
        weight: HEADER_FONT_WEIGHT,
        ..gpui::font(TABLE_FONT_FAMILY)
    };
    let header_size = HEADER_TEXT_SIZE.to_pixels(rem_size);
    // Always counted, so sorting a column does not change its width and shift the table.
    let indicator_width = SORT_INDICATOR_GAP.to_pixels(rem_size).as_f32()
        + ["↑", "↓"]
            .map(|arrow| line_width(window, &header_font, header_size, arrow))
            .into_iter()
            .fold(0.0, f32::max);
    let mut cell_measure = LineMeasure::new(window, gpui::font(TABLE_FONT_FAMILY), cell_size);
    let col_widths = compute_column_widths(
        rows,
        headers,
        |line| cell_measure.width(line),
        |line| line_width(window, &header_font, header_size, line) + indicator_width,
    );
    let digit_width = ('0'..='9')
        .map(|digit| cell_measure.char_width(digit))
        .fold(0.0, f32::max);
    let row_num_width = row_number_col_width(rows.row_count(), digit_width);
    (col_widths, row_num_width)
}

/// Measures cell lines without shaping most of them: a line of the characters
/// `measurable_by_pairs` accepts is as wide as its characters plus an adjustment for each
/// adjacent pair (kerning, and the space CoreText adds between CJK and Latin;
/// `pair_needs_shaping` skips the kanji and kana pairs, which have none). Characters and
/// pairs repeat across cells, so each is shaped once. Shaping each line whole takes about
/// 10-40µs, which adds up to a second on a file with hundreds of columns.
struct LineMeasure<'a> {
    window: &'a Window,
    font: Font,
    font_size: Pixels,
    char_widths: HashMap<char, f32>,
    pair_adjustments: HashMap<(char, char), f32>,
}

impl<'a> LineMeasure<'a> {
    fn new(window: &'a Window, font: Font, font_size: Pixels) -> Self {
        Self {
            window,
            font,
            font_size,
            char_widths: HashMap::new(),
            pair_adjustments: HashMap::new(),
        }
    }

    /// The width of `line`, or at least `MAX_COL_WIDTH` when it is wider: past that the column
    /// is as wide as it gets, so the rest is not measured.
    fn width(&mut self, line: &str) -> f32 {
        if !measurable_by_pairs(line) {
            return self.shaped_line_width(line);
        }
        let mut width = 0.0;
        let mut previous = None;
        for c in line.chars() {
            if width >= MAX_COL_WIDTH {
                break;
            }
            width += self.char_width(c);
            if let Some(previous) = previous {
                width += self.pair_adjustment(previous, c);
            }
            previous = Some(c);
        }
        width
    }

    fn char_width(&mut self, c: char) -> f32 {
        if let Some(&width) = self.char_widths.get(&c) {
            return width;
        }
        let width = self.shaped_width(c.encode_utf8(&mut [0; 4]));
        self.char_widths.insert(c, width);
        width
    }

    fn pair_adjustment(&mut self, a: char, b: char) -> f32 {
        if !pair_needs_shaping(a, b) {
            return 0.0;
        }
        if let Some(&adjustment) = self.pair_adjustments.get(&(a, b)) {
            return adjustment;
        }
        let pair: String = [a, b].into_iter().collect();
        let adjustment = self.shaped_width(&pair) - self.char_width(a) - self.char_width(b);
        self.pair_adjustments.insert((a, b), adjustment);
        adjustment
    }

    fn shaped_line_width(&self, line: &str) -> f32 {
        if let Some((prefix_end, _)) = line.char_indices().nth(SHAPED_PREFIX_CHARS) {
            let prefix_width = self.shaped_width(&line[..prefix_end]);
            if prefix_width >= MAX_COL_WIDTH {
                return prefix_width;
            }
        }
        self.shaped_width(line)
    }

    fn shaped_width(&self, text: &str) -> f32 {
        line_width(self.window, &self.font, self.font_size, text)
    }
}

/// The width of one line of text. `text` must not contain a newline.
fn line_width(window: &Window, font: &Font, font_size: Pixels, text: &str) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .layout_line(text, font_size, &[run], None)
        .width
        .as_f32()
}

impl Render for CsvrApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        // The bars draw their text themselves, so their height is one `text_sm` line, the size
        // the root sets for them.
        let input_line_height = {
            let mut style = window.text_style();
            style.font_size = CELL_TEXT_SIZE.into();
            style.line_height_in_pixels(window.rem_size())
        };
        let viewport_width = window.viewport_size().width.as_f32();
        let content_width = self.row_num_width
            + PINNED_BORDER_WIDTH
            + self
                .visible_col_indices
                .iter()
                .filter_map(|&col_idx| self.col_widths.get(col_idx))
                .sum::<f32>();
        let h_offset = px(clamp_h_offset(
            self.h_scroll_offset().as_f32(),
            content_width,
            viewport_width,
        ));
        let row_width = px(content_width.max(viewport_width));
        let filtered_count = self.filtered_indices.len();
        let total_count = self.rows.row_count();
        // The count is from the last result, so say when a newer one is still on its way.
        let rows_text = if self.is_filtering() {
            format!("{} / {} rows (filtering...)", filtered_count, total_count)
        } else {
            format!("{} / {} rows", filtered_count, total_count)
        };

        div()
            .track_focus(&self.focus_handle(cx))
            .key_context(if self.is_ime_editing() {
                "CsvrApp ime_editing"
            } else {
                "CsvrApp"
            })
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
            .on_action(cx.listener(|this, _: &ToggleMetadata, _window, cx| {
                this.toggle_metadata();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _window, cx| {
                this.copy_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &ExportJson, _window, cx| {
                this.export_json(cx);
            }))
            .on_action(cx.listener(|this, _: &ExportMarkdown, _window, cx| {
                this.export_markdown(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;

                // Arrow key navigation (works when no text input mode is active)
                if !keystroke.modifiers.platform
                    && !keystroke.modifiers.control
                    && !this.any_input_active()
                {
                    let viewport_width = window.viewport_size().width.as_f32();
                    match keystroke.key.as_str() {
                        "up" => {
                            this.move_selection(-1, 0, viewport_width);
                            cx.notify();
                            return;
                        }
                        "down" => {
                            this.move_selection(1, 0, viewport_width);
                            cx.notify();
                            return;
                        }
                        "left" => {
                            this.move_selection(0, -1, viewport_width);
                            cx.notify();
                            return;
                        }
                        "right" => {
                            this.move_selection(0, 1, viewport_width);
                            cx.notify();
                            return;
                        }
                        _ => {}
                    }
                }

                // --- `*` column filter, `&` row filter and `/` search input modes ---
                if this.col_filter_active || this.row_filter_active || this.search_active {
                    this.handle_text_input_key(keystroke, window, cx);
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

                // --- Normal mode: trigger keys for input modes ---
                if !keystroke.modifiers.platform && !keystroke.modifiers.control {
                    match keystroke.key_char.as_deref() {
                        Some("/") => this.toggle_search(),
                        Some("*") => this.toggle_col_filter(),
                        Some("&") => this.toggle_row_filter(),
                        Some("f") => this.toggle_pin_input(),
                        Some("m") => this.toggle_metadata(),
                        _ => return,
                    }
                    cx.notify();
                    // The input handler of a bar closed in this frame stays registered until
                    // the next draw, and would insert the trigger key into the bar opened here.
                    cx.stop_propagation();
                }
            }))
            .font_family(TABLE_FONT_FAMILY)
            .bg(rgb(BG_BASE))
            .text_color(rgb(TEXT_MAIN))
            .text_size(CELL_TEXT_SIZE)
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
                    .text_size(HEADER_TEXT_SIZE)
                    .font_weight(HEADER_FONT_WEIGHT)
                    // Inner row: pinned section + scrollable section
                    .child({
                        let pinned_count =
                            self.pinned_col_count.min(self.visible_col_indices.len());

                        let make_header_cell = |col_idx: usize, entity: Entity<CsvrApp>| {
                            let width = self.col_widths.get(col_idx).copied().unwrap_or(0.0);
                            let label = self
                                .headers
                                .get(col_idx)
                                .cloned()
                                .unwrap_or_else(|| "".into());
                            // Arrows rather than ▲▼: the browser build's bundled IBM Plex Sans has
                            // them, so sorting does not make it download the Japanese font. The
                            // native build uses the same arrows so both builds look alike.
                            let indicator: SharedString = match self.sort_state {
                                Some((c, SortDirection::Ascending)) if c == col_idx => "↑".into(),
                                Some((c, SortDirection::Descending)) if c == col_idx => "↓".into(),
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
                                        this.toggle_sort(col_idx, cx);
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
                                    el.child(
                                        div()
                                            .flex_shrink_0()
                                            .ml(SORT_INDICATOR_GAP)
                                            .child(indicator),
                                    )
                                })
                        };

                        // Pinned section: row number + pinned header columns, always fixed
                        let pinned_div = div()
                            .flex()
                            .flex_row()
                            .flex_shrink_0()
                            .border_r(px(PINNED_BORDER_WIDTH))
                            .border_color(rgb(BORDER_COLOR))
                            .py_1()
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

                        // Scrollable section: clipped the same way as in TableRow
                        let scrollable_div = div()
                            .flex_1()
                            .flex()
                            .flex_row()
                            .overflow_hidden()
                            .py_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_shrink_0()
                                    .ml(h_offset)
                                    .children(
                                        self.visible_col_indices.iter().skip(pinned_count).map(
                                            |&col_idx| make_header_cell(col_idx, entity.clone()),
                                        ),
                                    ),
                            );

                        div()
                            .flex()
                            .flex_row()
                            .child(pinned_div)
                            .child(scrollable_div)
                    }),
            )
            // Metadata section (rows from file header before the promoted data header)
            .when(self.metadata_visible && !self.metadata.is_empty(), |el| {
                let items: Vec<SharedString> = self
                    .metadata
                    .iter()
                    .filter_map(|row| {
                        let joined = row.join("  ");
                        let trimmed = joined.trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(SharedString::from(trimmed))
                        }
                    })
                    .collect();
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
                        .flex_wrap()
                        .gap_y_0p5()
                        .children(items.into_iter().enumerate().map(|(i, text)| {
                            let el = div().text_sm().text_color(rgb(TEXT_SUBTEXT)).child(text);
                            if i > 0 { el.ml_4() } else { el }
                        })),
                )
            })
            // `*` column filter bar
            .when(self.col_filter_active, |el| {
                let input = text_input_line(
                    self.text_input.as_ref(),
                    "Type column regex...",
                    if self.col_filter_error {
                        TEXT_ERROR
                    } else {
                        TEXT_MAIN
                    },
                    self.focus_handle.clone(),
                    entity.clone(),
                    input_line_height,
                );
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
                        .child(input)
                        .child(
                            div()
                                .text_color(rgb(if self.col_filter_error {
                                    TEXT_ERROR
                                } else {
                                    TEXT_SUBTEXT
                                }))
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
                let input = text_input_line(
                    self.text_input.as_ref(),
                    "Type row filter regex (or col:regex)...",
                    if self.row_filter_error {
                        TEXT_ERROR
                    } else {
                        TEXT_MAIN
                    },
                    self.focus_handle.clone(),
                    entity.clone(),
                    input_line_height,
                );
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
                        .child(input)
                        .child(
                            div()
                                .text_color(rgb(if self.row_filter_error {
                                    TEXT_ERROR
                                } else {
                                    TEXT_SUBTEXT
                                }))
                                .text_xs()
                                .child(if self.row_filter_error {
                                    "invalid regex".to_string()
                                } else {
                                    rows_text.clone()
                                }),
                        ),
                )
            })
            // Search bar
            .when(self.search_active, |el| {
                let input = text_input_line(
                    self.text_input.as_ref(),
                    "Type to search...",
                    TEXT_MAIN,
                    self.focus_handle.clone(),
                    entity.clone(),
                    input_line_height,
                );
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
                        .child(div().text_color(rgb(TEXT_SUBTEXT)).text_xs().child("/"))
                        .child(input)
                        .child(
                            div()
                                .text_color(rgb(TEXT_SUBTEXT))
                                .text_xs()
                                .child(rows_text.clone()),
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
                            .when(is_active, |el| {
                                el.bg(rgb(SURFACE1)).text_color(rgb(CHART_BLUE))
                            })
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
                            format!(
                                "Y: {}",
                                headers.get(chart_col).map(|s| s.as_ref()).unwrap_or("?")
                            )
                            .into()
                        } else {
                            headers
                                .get(chart_col)
                                .cloned()
                                .unwrap_or_else(|| "?".into())
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
                                        let cur_pos = numeric_cols2
                                            .iter()
                                            .position(|&c| c == this.chart_col)
                                            .unwrap_or(0);
                                        let next = (cur_pos + 1) % numeric_cols2.len();
                                        this.set_chart_col(numeric_cols2[next]);
                                        cx.notify();
                                    });
                                })
                                .child(col_label),
                        )
                        .when(is_scatter, |el| {
                            let x_label: SharedString = format!(
                                "X: {}",
                                headers.get(chart_x_col).map(|s| s.as_ref()).unwrap_or("?")
                            )
                            .into();
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
                                            let cur_pos = numeric_cols3
                                                .iter()
                                                .position(|&c| c == this.chart_x_col)
                                                .unwrap_or(0);
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
                let toolbar = toolbar.when(
                    chart_type == ChartType::Scatter && chart_col == chart_x_col,
                    |el| el.child(div().text_xs().text_color(rgb(CHART_PEACH)).child("X = Y")),
                );

                if !has_numeric || self.filtered_indices.is_empty() {
                    let message = if !has_numeric {
                        "No numeric columns"
                    } else {
                        "No data"
                    };
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
                            .child(div().text_color(rgb(TEXT_SUBTEXT)).text_sm().child(message)),
                    );
                }

                // Use cached chart data (recomputed on state changes, not every render)
                let (chart_data, chart_axes) = match &self.chart_data_cache {
                    Some(cached) => cached.clone(),
                    None => {
                        debug_assert!(
                            false,
                            "chart_data_cache is None when chart is active with data"
                        );
                        eprintln!("Bug: chart_data_cache is None when chart should have data");
                        (ChartData::Points(Vec::new()), points_axes(&[], false))
                    }
                };
                let header_name = |col: usize| headers.get(col).map(|s| s.as_ref()).unwrap_or("?");
                let legend =
                    chart_legend(chart_type, header_name(chart_col), header_name(chart_x_col));

                el.child(toolbar).child(
                    div()
                        .w_full()
                        .h(px(CHART_CANVAS_HEIGHT))
                        .bg(rgb(HEADER_BG))
                        .border_b_1()
                        .border_color(rgb(BORDER_COLOR))
                        .child(
                            canvas(
                                move |bounds, _window, _cx| {
                                    (bounds, chart_data, chart_axes, legend)
                                },
                                move |_bounds, (bounds, data, axes, legend), window, cx| {
                                    draw_chart(
                                        window, cx, bounds, &data, chart_type, &axes, &legend,
                                    );
                                },
                            )
                            .size_full(),
                        ),
                )
            })
            // Body
            .child(
                div().flex_1().size_full().child(
                    uniform_list(
                        "rows",
                        filtered_count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            this.visible_range = range.clone();
                            range
                                .filter_map(|i| {
                                    let original_idx = *this.filtered_indices.get(i)?;
                                    this.rows.row(original_idx)?;
                                    let selected_col =
                                        this.selected_cell.and_then(|(sel_row, col)| {
                                            if sel_row == i { Some(col) } else { None }
                                        });
                                    Some(TableRow {
                                        ix: i,
                                        rows: this.rows.clone(),
                                        row: original_idx,
                                        col_widths: this.col_widths.clone(),
                                        row_num_width: this.row_num_width,
                                        row_width,
                                        selected_col,
                                        entity: cx.entity(),
                                        visible_col_indices: this.visible_col_indices.clone(),
                                        pinned_col_count: this
                                            .pinned_col_count
                                            .min(this.visible_col_indices.len()),
                                        h_offset: this.h_scroll_offset(),
                                    })
                                })
                                .collect()
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .size_full()
                    .track_scroll(&self.scroll_handle),
                ),
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
                let (message, message_color) = match &self.status_message {
                    Some(message) => (Some(message.clone()), TEXT_MAIN),
                    None => (self.load_warning.clone(), TEXT_WARNING),
                };

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
                        let mut parts = vec![rows_text.clone()];
                        if vis_cols < total_cols {
                            parts.push(format!("{} / {} cols", vis_cols, total_cols));
                        }
                        if pinned > 0 {
                            parts.push(format!("f{}", pinned));
                        }
                        div().flex_shrink_0().child(parts.join("  "))
                    })
                    // The message and the stats shrink in proportion to their text, so a narrow
                    // window still shows part of both.
                    .when_some(message, |el, text| {
                        el.child(
                            div()
                                .flex_grow(1.0)
                                .mx_2()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .truncate()
                                .text_color(rgb(message_color))
                                .child(text),
                        )
                    })
                    .when_some(stats_text, |el, text| {
                        el.child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .truncate()
                                .text_color(rgb(TEXT_MAIN))
                                .child(text),
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

/// One line of the `/`, `*` or `&` bar, drawn from the text it is editing: the composing text is
/// underlined, the selection shaded, and the caret drawn where it is. It also registers the
/// input handler, which GPUI needs every frame to pass typed and composed text to the app.
fn text_input_line(
    input: Option<&TextInputState>,
    placeholder: &'static str,
    color: u32,
    focus_handle: FocusHandle,
    entity: Entity<CsvrApp>,
    line_height: Pixels,
) -> impl IntoElement {
    let text = input.map_or(String::new(), |input| input.text().to_string());
    let marked = input.and_then(TextInputState::marked_range);
    let selected = input.map_or(0..0, TextInputState::selected_range);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx);

            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let run = |len: usize, color: u32, underline: bool| TextRun {
                len,
                font: style.font(),
                color: rgb(color).into(),
                background_color: None,
                underline: underline.then(|| UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(rgb(color).into()),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let (shown, runs) = if text.is_empty() {
                (
                    placeholder.to_string(),
                    vec![run(placeholder.len(), TEXT_SUBTEXT, false)],
                )
            } else {
                // Runs split only at the composing text, so they never overlap; the selection is
                // shaded separately below.
                let runs = match &marked {
                    Some(marked) => [
                        (marked.start, false),
                        (marked.len(), true),
                        (text.len() - marked.end, false),
                    ]
                    .into_iter()
                    .filter(|&(len, _)| len > 0)
                    .map(|(len, underline)| run(len, color, underline))
                    .collect(),
                    None => vec![run(text.len(), color, false)],
                };
                (text.clone(), runs)
            };
            let line =
                window
                    .text_system()
                    .shape_line(SharedString::from(shown), font_size, &runs, None);

            let x_for = |index: usize| {
                if text.is_empty() {
                    px(0.0)
                } else {
                    line.x_for_index(index)
                }
            };
            let caret_x = x_for(selected.end);
            let offset = px(caret_scroll_offset(
                caret_x.as_f32(),
                CARET_WIDTH,
                bounds.size.width.as_f32(),
            ));
            let left = bounds.left() - offset;
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if !selected.is_empty() {
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            point(left + x_for(selected.start), bounds.top()),
                            point(left + x_for(selected.end), bounds.bottom()),
                        ),
                        rgb(SURFACE1),
                    ));
                }
                let origin = point(left, bounds.top());
                if let Err(e) = line.paint(origin, line_height, TextAlign::Left, None, window, cx) {
                    eprintln!("Warning: failed to paint input text: {e:?}");
                }
                if selected.is_empty() {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(left + caret_x, bounds.top()),
                            gpui::size(px(CARET_WIDTH), bounds.size.height),
                        ),
                        rgb(color),
                    ));
                }
            });
        },
    )
    .flex_1()
    .h(line_height)
}
