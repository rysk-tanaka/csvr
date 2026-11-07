use std::collections::{HashMap, HashSet};
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::data::{ChartAxes, ChartAxis, ChartType, CsvData, Row, SortDirection, Table};

/// Pixel width per character (monospace approximation at text_sm)
pub(crate) const CHAR_WIDTH: f32 = 7.5;
pub(crate) const MIN_COL_WIDTH: f32 = 50.0;
pub(crate) const MAX_COL_WIDTH: f32 = 400.0;
pub(crate) const COL_PADDING: f32 = 24.0;
pub(crate) const ROW_NUM_WIDTH: f32 = 16.0;

pub fn compute_column_widths(data: &CsvData) -> Vec<f32> {
    let sample_count = data.rows.row_count().min(100);
    data.headers
        .iter()
        .enumerate()
        .map(|(col_idx, header)| {
            let max_len = (0..sample_count)
                .map(|r| {
                    data.rows
                        .cell(r, col_idx)
                        .map_or(0, |cell| cell.chars().count())
                })
                .max()
                .unwrap_or(0)
                .max(header.to_uppercase().chars().count());
            (max_len as f32 * CHAR_WIDTH + COL_PADDING).clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
        })
        .collect()
}

pub fn row_number_col_width(total_rows: usize) -> f32 {
    let digits = total_rows.max(1).ilog10() as usize + 1;
    (digits as f32 * CHAR_WIDTH + ROW_NUM_WIDTH).max(40.0)
}

/// The list clamps its scroll offset only during prepaint, after the header has been built,
/// so the header needs the same limit to stay aligned at the right edge.
pub fn clamp_h_offset(offset: f32, content_width: f32, viewport_width: f32) -> f32 {
    let max_scroll = (content_width - viewport_width).max(0.0);
    offset.clamp(-max_scroll, 0.0)
}

/// Returns the scroll offset that brings a column into the area right of the pinned section,
/// moving as little as possible. A column wider than the area aligns to its left edge so the
/// start of the value stays visible.
pub fn h_offset_to_reveal(offset: f32, col_start: f32, col_width: f32, visible_width: f32) -> f32 {
    // The pinned section can fill the window, leaving no area to scroll the column into.
    if visible_width <= 0.0 {
        return offset;
    }
    let col_left = col_start + offset;
    if col_left < 0.0 || col_width >= visible_width {
        return -col_start;
    }
    let overflow = col_left + col_width - visible_width;
    if overflow > 0.0 {
        offset - overflow
    } else {
        offset
    }
}

/// Where each original row sits in a filtered order. A filter task builds it off the UI thread,
/// so the result can be anchored to whatever the view shows when it arrives with lookups only,
/// even if the user scrolled or changed the selection while the task ran.
pub struct RowPositions {
    /// Indexed by original row; `NOT_SHOWN` for rows the order leaves out. `u32` halves the
    /// size, since this has an entry for every row in the file and is rebuilt on each keystroke.
    positions: Vec<u32>,
    len: usize,
}

impl RowPositions {
    const NOT_SHOWN: u32 = u32::MAX;

    pub fn new(order: &[usize], row_count: usize) -> Self {
        let mut positions = vec![Self::NOT_SHOWN; row_count];
        for (pos, &row) in order.iter().enumerate() {
            // Positions past u32 would need more rows than fit in memory as strings.
            let Ok(pos) = u32::try_from(pos) else { break };
            if let Some(slot) = positions.get_mut(row) {
                *slot = pos;
            }
        }
        Self {
            positions,
            len: order.len(),
        }
    }

    /// Position of `row` in the order, or `None` if the order leaves it out.
    pub fn get(&self, row: usize) -> Option<usize> {
        self.positions
            .get(row)
            .copied()
            .filter(|&pos| pos != Self::NOT_SHOWN)
            .map(|pos| pos as usize)
    }
}

/// Position in `new` to show at the top of the view when the filtered rows change from `old`:
/// the first row at or after `old[top]` that is still in `new`, or the last row of `new` if
/// there is none. Returns `None` if either list is empty, since there is nothing to keep.
pub fn anchored_position(old: &[usize], new: &RowPositions, top: usize) -> Option<usize> {
    if old.is_empty() {
        return None;
    }
    old.iter()
        .skip(top)
        .find_map(|&row| new.get(row))
        .or_else(|| new.len.checked_sub(1))
}

/// The `/` query, matched as a literal anywhere in a cell regardless of case.
enum SearchQuery {
    /// No `/` query: every row passes.
    None,
    /// The escaped query, so that cells are matched without allocating a lowercased copy of each.
    Regex(Regex),
    /// Used only when the regex cannot be built (a query past the regex size limit), so that a
    /// long query still narrows the rows instead of matching nothing.
    Lowercase(String),
}

impl SearchQuery {
    /// `size_limit` is `None` to keep the `regex` crate's default; tests pass a small one to
    /// reach the fallback.
    fn new(query: &str, size_limit: Option<usize>) -> Self {
        if query.is_empty() {
            return Self::None;
        }
        let mut builder = RegexBuilder::new(&regex::escape(query));
        builder.case_insensitive(true);
        if let Some(limit) = size_limit {
            builder.size_limit(limit);
        }
        match builder.build() {
            Ok(re) => Self::Regex(re),
            Err(_) => Self::Lowercase(query.to_lowercase()),
        }
    }

    fn matches(&self, row: Row<'_>) -> bool {
        match self {
            Self::None => true,
            Self::Regex(re) => row.iter().any(|cell| re.is_match(cell)),
            Self::Lowercase(query) => row
                .iter()
                .any(|cell| cell.to_lowercase().contains(query.as_str())),
        }
    }
}

/// The `/` search and the `&` regex filter, prepared once per query so that the rows can be
/// checked in chunks off the UI thread.
pub struct RowFilter {
    query: SearchQuery,
    /// Compiled `&` pattern and its target column; `None` when the pattern is empty or invalid.
    regex: Option<(Regex, Option<usize>)>,
}

impl RowFilter {
    /// An invalid `pattern` is ignored so that the `/` search still narrows the rows; check it
    /// with `is_valid_pattern` to show the error.
    pub fn new(query: &str, pattern: &str, target_col: Option<usize>) -> Self {
        Self::with_search(SearchQuery::new(query, None), pattern, target_col)
    }

    fn with_search(query: SearchQuery, pattern: &str, target_col: Option<usize>) -> Self {
        let regex = if pattern.is_empty() {
            None
        } else {
            Self::build_regex(pattern).ok().map(|re| (re, target_col))
        };
        Self { query, regex }
    }

    /// Whether the `&` filter accepts `pattern`. An empty pattern is valid and filters nothing.
    pub fn is_valid_pattern(pattern: &str) -> bool {
        pattern.is_empty() || Self::build_regex(pattern).is_ok()
    }

    /// The one place the `&` regex options are set, so validation and filtering agree.
    fn build_regex(pattern: &str) -> Result<Regex, regex::Error> {
        RegexBuilder::new(pattern).case_insensitive(true).build()
    }

    /// The two conditions are checked separately, so they may hold in different cells.
    pub(crate) fn matches(&self, row: Row<'_>) -> bool {
        if !self.query.matches(row) {
            return false;
        }
        match &self.regex {
            None => true,
            Some((re, Some(col))) => row.get(*col).is_some_and(|cell| re.is_match(cell)),
            Some((re, None)) => row.iter().any(|cell| re.is_match(cell)),
        }
    }

    /// Append the indices in `range` whose rows match. Called chunk by chunk so that a task can
    /// yield between chunks.
    pub fn filter_range(&self, rows: &Table, range: Range<usize>, out: &mut Vec<usize>) {
        let end = range.end.min(rows.row_count());
        let start = range.start.min(end);
        out.extend((start..end).filter(|&i| rows.row(i).is_some_and(|row| self.matches(row))));
    }
}

/// Determine which columns are numeric based on all rows.
pub fn compute_numeric_columns(rows: &Table, col_count: usize) -> Vec<bool> {
    (0..col_count)
        .map(|col| {
            let mut numeric_count: usize = 0;
            let mut non_empty_count: usize = 0;
            for r in 0..rows.row_count() {
                let val = rows.cell(r, col).unwrap_or("");
                if !val.is_empty() {
                    non_empty_count += 1;
                    if val.parse::<f64>().is_ok() {
                        numeric_count += 1;
                    }
                }
            }
            // Treat as numeric if more than half of non-empty values are parseable
            non_empty_count > 0 && numeric_count > non_empty_count / 2
        })
        .collect()
}

/// Reorder rows already filtered for the current queries, for a sort change alone. Starts from
/// file order, as a full refilter does, so rows that tie in the sort column keep file order
/// instead of the order of the previous sort. With no sort, the result is file order.
pub fn reorder_indices(
    rows: &Table,
    shown: &[usize],
    sort: Option<(usize, bool, SortDirection)>,
) -> Vec<usize> {
    let mut file_order = shown.to_vec();
    file_order.sort_unstable();
    match sort {
        Some((col, use_numeric, direction)) => {
            sort_indices(rows, &file_order, col, use_numeric, direction)
        }
        None => file_order,
    }
}

pub fn sort_indices(
    rows: &Table,
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
                let val = rows.cell(i, col).unwrap_or("");
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
            let val_a = rows.cell(a, col).unwrap_or("");
            let val_b = rows.cell(b, col).unwrap_or("");
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
pub fn extract_column_values(rows: &Table, indices: &[usize], col: usize) -> Vec<(usize, f64)> {
    indices
        .iter()
        .filter_map(|&i| {
            let val = rows.cell(i, col)?;
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
pub fn downsample<T: Copy>(values: &[T], max: usize) -> Vec<T> {
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

/// Extract paired finite numeric values from two columns for the given row indices.
/// Only rows where both columns have a valid finite number are included.
pub fn extract_scatter_pairs(
    rows: &Table,
    indices: &[usize],
    x_col: usize,
    y_col: usize,
) -> Vec<(f64, f64)> {
    indices
        .iter()
        .filter_map(|&i| {
            let row = rows.row(i)?;
            let x = row
                .get(x_col)?
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())?;
            let y = row
                .get(y_col)?
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())?;
            Some((x, y))
        })
        .collect()
}

/// Summary statistics for a numeric column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStats {
    pub count: usize,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub stddev: f64,
}

/// Compute summary statistics for a numeric column over the given row indices.
/// Collects values into a Vec for median (requires sorting) and stddev (two-pass).
/// Returns `None` if no finite numeric values exist.
pub fn compute_column_stats(rows: &Table, indices: &[usize], col: usize) -> Option<ColumnStats> {
    let mut values: Vec<f64> = Vec::new();
    let mut sum: f64 = 0.0;
    let mut min: f64 = f64::INFINITY;
    let mut max: f64 = f64::NEG_INFINITY;

    for &row_idx in indices {
        let v = rows
            .cell(row_idx, col)
            .and_then(|cell| cell.parse::<f64>().ok())
            .filter(|v| v.is_finite());
        if let Some(v) = v {
            values.push(v);
            sum += v;
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
        }
    }

    if values.is_empty() {
        return None;
    }

    let count = values.len();
    let mean = sum / count as f64;

    // Sample standard deviation (Bessel's correction: divide by n-1).
    // For count == 1, variance is 0.0 (no spread in a single observation).
    let variance = if count == 1 {
        0.0
    } else {
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64
    };
    let stddev = variance.sqrt();

    values.sort_by(|a, b| a.total_cmp(b));
    let median = if count % 2 == 1 {
        values[count / 2]
    } else {
        (values[count / 2 - 1] + values[count / 2]) / 2.0
    };

    Some(ColumnStats {
        count,
        sum,
        min,
        max,
        mean,
        median,
        stddev,
    })
}

/// Return indices of columns whose header matches the regex pattern (case-insensitive).
/// Empty pattern returns all column indices.
pub fn filter_columns_by_regex(
    headers: &[String],
    pattern: &str,
) -> Result<Vec<usize>, regex::Error> {
    if pattern.is_empty() {
        return Ok((0..headers.len()).collect());
    }
    let re = RegexBuilder::new(pattern).case_insensitive(true).build()?;
    Ok(headers
        .iter()
        .enumerate()
        .filter(|(_, h)| re.is_match(h))
        .map(|(i, _)| i)
        .collect())
}

/// Parse column filter query: "col:pattern" returns (Some(col_index), pattern),
/// plain "pattern" returns (None, pattern). If col name doesn't match any header,
/// treats the whole string as a global pattern.
pub fn parse_column_filter(query: &str, headers: &[String]) -> (Option<usize>, String) {
    if let Some((prefix, suffix)) = query.split_once(':')
        && let Some(idx) = headers.iter().position(|h| h.eq_ignore_ascii_case(prefix))
    {
        return (Some(idx), suffix.to_string());
    }
    (None, query.to_string())
}

/// Compute histogram bin counts for the given values.
pub fn compute_histogram_bins(values: &[f64], bin_count: usize) -> Vec<usize> {
    if values.is_empty() || bin_count == 0 {
        return vec![0; bin_count];
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range.abs() < f64::EPSILON {
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

/// Format an axis label with enough digits to tell it from the next tick, half of `range` away.
pub(crate) fn format_tick(value: f64, range: f64) -> String {
    let step = range / 2.0;
    if value.abs() >= 1e12 {
        let precision = ((value.abs() / step).log10().ceil() + 1.0).clamp(3.0, 16.0) as usize;
        return format!("{value:.precision$e}");
    }
    let decimals = (1.0 - step.log10()).ceil().clamp(0.0, 17.0) as usize;
    let label = format!("{value:.decimals$}");
    // Rounding a small negative value leaves "-0".
    match label.strip_prefix('-') {
        Some(positive) if positive.parse::<f64>() == Ok(0.0) => positive.to_string(),
        _ => label,
    }
}

fn value_bounds(values: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let (min, max) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), v| {
        (min.min(v), max.max(v))
    });
    (min <= max).then_some((min, max))
}

/// Width of the range for all-equal values. It grows with the value because adding 1.0 to a value
/// like 1e16 would leave it unchanged, and the plot divides by this width.
fn equal_values_width(value: f64) -> f64 {
    (value.abs() * 1e-9).max(1.0)
}

fn empty_axis() -> ChartAxis {
    ChartAxis {
        lo: 0.0,
        hi: 1.0,
        ticks: Vec::new(),
    }
}

fn value_axis(lo: f64, hi: f64) -> ChartAxis {
    let range = hi - lo;
    ChartAxis {
        lo,
        hi,
        ticks: [(0.0, lo), (0.5, (lo + hi) / 2.0), (1.0, hi)]
            .into_iter()
            .map(|(pos, value)| (pos, format_tick(value, range)))
            .collect(),
    }
}

fn axis_from_bounds(bounds: Option<(f64, f64)>) -> ChartAxis {
    let Some((lo, hi)) = bounds else {
        return empty_axis();
    };
    if (hi - lo).abs() < f64::EPSILON {
        return value_axis(lo, lo + equal_values_width(lo));
    }
    value_axis(lo, hi)
}

/// Axes for bar and line charts. Bars start from zero when all values are positive.
pub fn points_axes(values: &[f64], include_zero: bool) -> ChartAxes {
    let bounds = value_bounds(values.iter().copied())
        .map(|(min, max)| (if include_zero { min.min(0.0) } else { min }, max));
    ChartAxes {
        y: axis_from_bounds(bounds),
        x: None,
    }
}

pub fn pairs_axes(pairs: &[(f64, f64)]) -> ChartAxes {
    ChartAxes {
        y: axis_from_bounds(value_bounds(pairs.iter().map(|p| p.1))),
        x: Some(axis_from_bounds(value_bounds(pairs.iter().map(|p| p.0)))),
    }
}

pub fn histogram_axes(values: &[f64], bins: &[usize]) -> ChartAxes {
    let max_count = bins.iter().copied().max().unwrap_or(0);
    let y = if max_count == 0 {
        empty_axis()
    } else {
        // Counts are whole numbers, so skip the middle tick when it would fall between two.
        let mut ticks = vec![(0.0, "0".to_string())];
        if max_count % 2 == 0 {
            ticks.push((0.5, (max_count / 2).to_string()));
        }
        ticks.push((1.0, max_count.to_string()));
        ChartAxis {
            lo: 0.0,
            hi: max_count as f64,
            ticks,
        }
    };
    let x = match value_bounds(values.iter().copied()) {
        None => empty_axis(),
        // compute_histogram_bins puts all-equal values in the middle bin.
        Some((min, max)) if (max - min).abs() < f64::EPSILON => {
            let half_width = equal_values_width(min) / 2.0;
            value_axis(min - half_width, min + half_width)
        }
        Some((min, max)) => value_axis(min, max),
    };
    ChartAxes { y, x: Some(x) }
}

pub fn chart_legend(chart_type: ChartType, y_name: &str, x_name: &str) -> String {
    // shape_line expects a single line.
    let single_line = |name: &str| name.replace(['\r', '\n'], " ");
    match chart_type {
        ChartType::Scatter => format!("Y: {}  X: {}", single_line(y_name), single_line(x_name)),
        ChartType::Bar | ChartType::Line | ChartType::Histogram => single_line(y_name),
    }
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Build deduplicated JSON keys for visible columns.
/// Duplicate headers get `_2`, `_3`, … suffixes.
/// Increments the suffix until the candidate is unique among all emitted keys.
fn deduplicate_json_keys(headers: &[String], visible_col_indices: &[usize]) -> Vec<String> {
    let mut seen_counts: HashMap<&str, usize> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut keys = Vec::with_capacity(visible_col_indices.len());
    for &col_idx in visible_col_indices {
        let name = headers.get(col_idx).map_or("", |s| s.as_str());
        let count = seen_counts.entry(name).or_insert(0);
        *count += 1;
        let key = if *count == 1 && !used.contains(name) {
            name.to_string()
        } else {
            let mut n = *count;
            loop {
                let candidate = format!("{}_{}", name, n);
                if !used.contains(&candidate) {
                    break candidate;
                }
                n += 1;
            }
        };
        used.insert(key.clone());
        keys.push(key);
    }
    keys
}

pub fn copied_status_message(format_name: &str, row_count: usize) -> String {
    let unit = if row_count == 1 { "row" } else { "rows" };
    format!("Copied as {format_name} ({row_count} {unit})")
}

/// Export visible data as JSON (array of objects).
pub fn export_json(
    headers: &[String],
    rows: &Table,
    filtered_indices: &[usize],
    visible_col_indices: &[usize],
) -> String {
    if visible_col_indices.is_empty() {
        return "[]\n".to_string();
    }
    let deduped_keys = deduplicate_json_keys(headers, visible_col_indices);
    let mut out = String::new();
    let mut written = 0usize;
    for &row_idx in filtered_indices {
        let Some(row) = rows.row(row_idx) else {
            continue;
        };
        if written > 0 {
            out.push_str(",\n");
        }
        out.push_str("  {");
        for (j, &col_idx) in visible_col_indices.iter().enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            let key = &deduped_keys[j];
            let val = row.get(col_idx).unwrap_or("");
            out.push_str(&format!(
                "\"{}\": \"{}\"",
                escape_json_string(key),
                escape_json_string(val)
            ));
        }
        out.push('}');
        written += 1;
    }
    if written == 0 {
        return "[]\n".to_string();
    }
    format!("[\n{}\n]\n", out)
}

fn escape_markdown_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Export visible data as GitHub-flavored Markdown table.
pub fn export_markdown(
    headers: &[String],
    rows: &Table,
    filtered_indices: &[usize],
    visible_col_indices: &[usize],
) -> String {
    if visible_col_indices.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    // Header row
    out.push('|');
    for &col_idx in visible_col_indices {
        let h = headers.get(col_idx).map_or("", |s| s.as_str());
        out.push_str(&format!(" {} |", escape_markdown_cell(h)));
    }
    out.push('\n');
    // Separator row
    out.push('|');
    for _ in visible_col_indices {
        out.push_str(" --- |");
    }
    out.push('\n');
    // Data rows
    for &row_idx in filtered_indices {
        let Some(row) = rows.row(row_idx) else {
            continue;
        };
        out.push('|');
        for &col_idx in visible_col_indices {
            let val = row.get(col_idx).unwrap_or("");
            out.push_str(&format!(" {} |", escape_markdown_cell(val)));
        }
        out.push('\n');
    }
    out
}

/// The horizontal shift that keeps the caret inside a single-line input of `width`: none while
/// the caret fits, otherwise enough to put the caret at the right edge.
pub fn caret_scroll_offset(caret_x: f32, caret_width: f32, width: f32) -> f32 {
    (caret_x + caret_width - width).max(0.0)
}

/// The text being edited in an input bar, with the selection and the IME's composing range.
/// Ranges are byte offsets on char boundaries; the `_utf16` methods take and return UTF-16
/// offsets, which is what platform text input uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputState {
    text: String,
    selected: Range<usize>,
    marked: Option<Range<usize>>,
    /// Set by `replace_and_mark` even when the composing text becomes empty, and cleared by
    /// `replace` / `unmark` / `delete_backward`. The browser keeps composing until compositionend in that case.
    composition_open: bool,
}

impl TextInputState {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let end = text.len();
        Self {
            text,
            selected: end..end,
            marked: None,
            composition_open: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte range of the selection; empty when it is only a caret.
    pub fn selected_range(&self) -> Range<usize> {
        self.selected.clone()
    }

    /// Byte range of the composing text, for drawing its underline.
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    pub fn is_composing(&self) -> bool {
        self.marked.is_some()
    }

    pub fn is_composition_open(&self) -> bool {
        self.composition_open
    }

    pub fn len_utf16(&self) -> usize {
        self.text.encode_utf16().count()
    }

    pub fn selected_range_utf16(&self) -> Range<usize> {
        self.range_to_utf16(&self.selected)
    }

    pub fn marked_range_utf16(&self) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    /// Returns the text in `range_utf16` and the UTF-16 range it actually covers after the
    /// range is fitted to the text.
    pub fn text_for_range_utf16(&self, range_utf16: Range<usize>) -> (String, Range<usize>) {
        let range = self.range_from_utf16(&range_utf16);
        (
            self.text[range.clone()].to_string(),
            self.range_to_utf16(&range),
        )
    }

    /// Commits `new_text` over `range_utf16`, or over the composing text or the selection when
    /// no range is given. The caret ends up right after the inserted text.
    pub fn replace(&mut self, range_utf16: Option<Range<usize>>, new_text: &str) {
        let range = self.edit_range(range_utf16);
        let inserted_end = self.splice(range, new_text);
        self.selected = inserted_end..inserted_end;
        self.marked = None;
        self.composition_open = false;
    }

    /// Puts `new_text` over the range like `replace`, and marks it as the composing text.
    /// `new_selected_utf16` is relative to the start of the inserted text.
    pub fn replace_and_mark(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_utf16: Option<Range<usize>>,
    ) {
        let range = self.edit_range(range_utf16);
        let inserted_end = self.splice(range.clone(), new_text);
        let inserted = range.start..inserted_end;
        self.marked = (!inserted.is_empty()).then(|| inserted.clone());
        self.selected = match new_selected_utf16 {
            Some(selected) => {
                let start_utf16 = self.offset_to_utf16(inserted.start);
                let inserted_len_utf16 = self.offset_to_utf16(inserted.end) - start_utf16;
                let relative = order(selected);
                self.range_from_utf16(
                    &(start_utf16 + relative.start.min(inserted_len_utf16)
                        ..start_utf16 + relative.end.min(inserted_len_utf16)),
                )
            }
            None => inserted_end..inserted_end,
        };
        self.composition_open = true;
    }

    /// Ends composing and keeps the composing text as committed text where it is.
    pub fn unmark(&mut self) {
        self.marked = None;
        self.composition_open = false;
    }

    pub fn set_selected_range_utf16(&mut self, range_utf16: Range<usize>) {
        self.selected = self.range_from_utf16(&range_utf16);
    }

    pub fn select_all(&mut self) {
        // The IME owns the selection while composing.
        if self.is_composing() {
            return;
        }
        self.selected = 0..self.text.len();
    }

    /// Deletes the selection, or the char before the caret when the selection is empty.
    pub fn delete_backward(&mut self) {
        let range = if self.selected.is_empty() {
            let end = self.selected.start;
            let start = self.text[..end]
                .char_indices()
                .next_back()
                .map_or(end, |(index, _)| index);
            start..end
        } else {
            self.selected.clone()
        };
        self.text.replace_range(range.clone(), "");
        self.selected = range.start..range.start;
        self.marked = None;
        self.composition_open = false;
    }

    fn edit_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| self.range_from_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone())
    }

    /// Replaces `range` with `new_text` and returns where the inserted text ends.
    fn splice(&mut self, range: Range<usize>, new_text: &str) -> usize {
        // The bars hold a one-line query, and shaping a line rejects newlines. Each newline
        // becomes one space, so offsets the platform computed from `new_text` stay valid.
        let new_text = new_text.replace(['\n', '\r'], " ");
        self.text.replace_range(range.clone(), &new_text);
        range.start + new_text.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset].encode_utf16().count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// Fits a UTF-16 offset to the text, rounding down to a char boundary so an offset inside a
    /// surrogate pair cannot split a char.
    fn offset_from_utf16(&self, offset_utf16: usize) -> usize {
        let mut utf16_count = 0;
        for (index, ch) in self.text.char_indices() {
            if utf16_count + ch.len_utf16() > offset_utf16 {
                return index;
            }
            utf16_count += ch.len_utf16();
        }
        self.text.len()
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let range = order(range_utf16.clone());
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

fn order(range: Range<usize>) -> Range<usize> {
    if range.start <= range.end {
        range
    } else {
        range.end..range.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::CsvData;

    fn make_csv_data(headers: &[&str], rows: &[&[&str]]) -> CsvData {
        CsvData {
            headers: headers.iter().map(|s| s.to_string()).collect(),
            rows: Table::from_rows(rows),
            metadata: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn make_rows(rows: &[&[&str]]) -> Table {
        Table::from_rows(rows)
    }

    // --- clamp_h_offset ---

    #[test]
    fn clamp_h_offset_keeps_offset_in_range() {
        assert_eq!(clamp_h_offset(-100.0, 1000.0, 800.0), -100.0);
    }

    #[test]
    fn clamp_h_offset_stops_at_right_edge() {
        assert_eq!(clamp_h_offset(-500.0, 1000.0, 800.0), -200.0);
    }

    #[test]
    fn clamp_h_offset_is_zero_when_content_fits() {
        assert_eq!(clamp_h_offset(-100.0, 500.0, 800.0), 0.0);
    }

    #[test]
    fn clamp_h_offset_stops_at_left_edge() {
        assert_eq!(clamp_h_offset(10.0, 1000.0, 800.0), 0.0);
    }

    // --- h_offset_to_reveal ---

    #[test]
    fn h_offset_to_reveal_keeps_offset_when_visible() {
        assert_eq!(h_offset_to_reveal(0.0, 100.0, 50.0, 300.0), 0.0);
    }

    #[test]
    fn h_offset_to_reveal_keeps_offset_at_left_edge() {
        assert_eq!(h_offset_to_reveal(-100.0, 100.0, 50.0, 300.0), -100.0);
    }

    #[test]
    fn h_offset_to_reveal_keeps_offset_at_right_edge() {
        assert_eq!(h_offset_to_reveal(0.0, 250.0, 50.0, 300.0), 0.0);
    }

    #[test]
    fn h_offset_to_reveal_scrolls_right_for_overflow() {
        assert_eq!(h_offset_to_reveal(0.0, 350.0, 50.0, 300.0), -100.0);
    }

    #[test]
    fn h_offset_to_reveal_scrolls_left_for_hidden_column() {
        assert_eq!(h_offset_to_reveal(-200.0, 100.0, 50.0, 300.0), -100.0);
    }

    #[test]
    fn h_offset_to_reveal_aligns_wide_column_to_left_edge() {
        assert_eq!(h_offset_to_reveal(-50.0, 100.0, 400.0, 300.0), -100.0);
    }

    #[test]
    fn h_offset_to_reveal_keeps_offset_without_visible_area() {
        assert_eq!(h_offset_to_reveal(-50.0, 100.0, 50.0, 0.0), -50.0);
    }

    // --- compute_column_widths ---

    #[test]
    fn column_widths_basic() {
        let data = make_csv_data(&["name", "age"], &[&["Alice", "30"], &["Bob", "25"]]);
        let widths = compute_column_widths(&data);
        assert_eq!(widths.len(), 2);
        assert!((widths[0] - 61.5).abs() < 0.01);
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
        assert!((widths[0] - 54.0).abs() < 0.01);
    }

    #[test]
    fn column_widths_header_longer_than_data() {
        let data = make_csv_data(&["description"], &[&["hi"]]);
        let widths = compute_column_widths(&data);
        assert!((widths[0] - 106.5).abs() < 0.01);
    }

    #[test]
    fn parse_ragged_rows() {
        // flexible(true) accepts rows with fewer/more fields than the header.
        // Ragged rows display with blank cells in the UI.
        let input = "a,b,c\n1,2\n4,5,6\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.rows.row_count(), 2);
        assert_eq!(
            data.rows.row(0).unwrap().iter().collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            data.rows.row(1).unwrap().iter().collect::<Vec<_>>(),
            ["4", "5", "6"]
        );
    }

    #[test]
    fn column_widths_short_rows() {
        let data = make_csv_data(&["a", "b", "c"], &[&["x", "y"]]);
        let widths = compute_column_widths(&data);
        assert_eq!(widths.len(), 3);
        assert!((widths[2] - MIN_COL_WIDTH).abs() < 0.01);
    }

    // --- row_number_col_width ---

    #[test]
    fn row_num_width_zero_rows() {
        let w = row_number_col_width(0);
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
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_two_digits() {
        let w = row_number_col_width(99);
        assert!((w - 40.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_four_digits() {
        let w = row_number_col_width(9999);
        assert!((w - 46.0).abs() < 0.01);
    }

    #[test]
    fn row_num_width_large() {
        let w = row_number_col_width(1_000_000);
        assert!((w - 68.5).abs() < 0.01);
    }

    // --- RowFilter: `/` search ---

    /// Filter all rows in one call, returning the matching indices and the regex error flag.
    fn filter_all(
        rows: &Table,
        query: &str,
        pattern: &str,
        target_col: Option<usize>,
    ) -> (Vec<usize>, bool) {
        let filter = RowFilter::new(query, pattern, target_col);
        let has_error = !RowFilter::is_valid_pattern(pattern);
        let mut out = Vec::new();
        filter.filter_range(rows, 0..rows.row_count(), &mut out);
        (out, has_error)
    }

    fn search(rows: &Table, query: &str) -> Vec<usize> {
        filter_all(rows, query, "", None).0
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let rows = make_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        assert_eq!(search(&rows, ""), vec![0, 1]);
    }

    #[test]
    fn filter_matches_substring() {
        let rows = make_rows(&[&["Alice", "Tokyo"], &["Bob", "Osaka"], &["Carol", "Tokyo"]]);
        assert_eq!(search(&rows, "tokyo"), vec![0, 2]);
    }

    #[test]
    fn filter_case_insensitive() {
        let rows = make_rows(&[&["ALICE"], &["alice"], &["Alice"]]);
        assert_eq!(search(&rows, "alice"), vec![0, 1, 2]);
    }

    #[test]
    fn filter_no_matches() {
        let rows = make_rows(&[&["Alice"], &["Bob"]]);
        assert!(search(&rows, "xyz").is_empty());
    }

    #[test]
    fn filter_matches_any_column() {
        let rows = make_rows(&[&["Alice", "30", "Tokyo"], &["Bob", "25", "Osaka"]]);
        assert_eq!(search(&rows, "25"), vec![1]);
    }

    #[test]
    fn filter_empty_rows() {
        let rows = make_rows(&[]);
        assert!(search(&rows, "test").is_empty());
    }

    #[test]
    fn filter_query_is_literal() {
        let rows = make_rows(&[&["axb"], &["a.b"], &["a+b"]]);
        assert_eq!(search(&rows, "a.b"), vec![1]);
        assert_eq!(search(&rows, "a+b"), vec![2]);
    }

    #[test]
    fn filter_query_folds_final_sigma() {
        // `to_lowercase` turns a word-final Σ into ς, which σ would not find.
        let rows = make_rows(&[&["ΟΔΟΣ"], &["ΑΒΓ"]]);
        assert_eq!(search(&rows, "σ"), vec![0]);
    }

    #[test]
    fn filter_query_falls_back_when_regex_is_too_large() {
        let query = SearchQuery::new("alice", Some(1));
        assert!(matches!(query, SearchQuery::Lowercase(_)));
        let rows = make_rows(&[&["ALICE", "Tokyo"], &["Alice", "Osaka"], &["Bob", "Tokyo"]]);
        let filter = RowFilter::with_search(query, "tokyo", None);
        let mut out = Vec::new();
        filter.filter_range(&rows, 0..rows.row_count(), &mut out);
        assert_eq!(out, vec![0]);
    }

    // --- RowFilter: `/` and `&` together, and chunking ---

    #[test]
    fn row_filter_combines_search_and_regex() {
        let rows = make_rows(&[&["Alice", "Tokyo"], &["Alice", "Osaka"], &["Bob", "Tokyo"]]);
        assert_eq!(filter_all(&rows, "alice", "tokyo", None), (vec![0], false));
    }

    #[test]
    fn row_filter_conditions_may_hold_in_different_cells() {
        let rows = make_rows(&[&["Alice", "Tokyo"]]);
        assert_eq!(filter_all(&rows, "Alice", "Tokyo", None), (vec![0], false));
    }

    #[test]
    fn row_filter_target_col_applies_to_regex_only() {
        // "alice" is found in column 0 even though the regex targets column 1.
        let rows = make_rows(&[&["Alice", "Tokyo"], &["Tokyo", "Alice"]]);
        assert_eq!(
            filter_all(&rows, "alice", "tokyo", Some(1)),
            (vec![0], false)
        );
    }

    #[test]
    fn row_filter_invalid_regex_keeps_search() {
        let rows = make_rows(&[&["Alice"], &["Bob"]]);
        assert_eq!(filter_all(&rows, "bob", "[bad", None), (vec![1], true));
    }

    #[test]
    fn row_filter_range_matches_whole_scan() {
        let rows = make_rows(&[
            &["a1"],
            &["b2"],
            &["a3"],
            &["a4"],
            &["b5"],
            &["a6"],
            &["a7"],
        ]);
        let filter = RowFilter::new("a", "", None);
        let mut chunked = Vec::new();
        for start in (0..rows.row_count()).step_by(3) {
            filter.filter_range(&rows, start..start + 3, &mut chunked);
        }
        assert_eq!(chunked, search(&rows, "a"));
    }

    #[test]
    fn row_filter_range_beyond_rows_is_clamped() {
        let rows = make_rows(&[&["a"], &["a"]]);
        let filter = RowFilter::new("a", "", None);
        let mut out = Vec::new();
        filter.filter_range(&rows, 1..10, &mut out);
        filter.filter_range(&rows, 5..10, &mut out);
        assert_eq!(out, vec![1]);
    }

    // --- reorder_indices ---

    #[test]
    fn reorder_ties_keep_file_order_across_sorts() {
        // Sorting by column 0 puts row 2 before row 1; a later sort by column 1 must not keep
        // that order for the rows tied at "1".
        let rows = make_rows(&[&["x", "2"], &["y", "1"], &["x", "1"]]);
        let by_col0 = reorder_indices(
            &rows,
            &[0, 1, 2],
            Some((0, false, SortDirection::Ascending)),
        );
        assert_eq!(by_col0, vec![0, 2, 1]);
        let by_col1 = reorder_indices(&rows, &by_col0, Some((1, true, SortDirection::Ascending)));
        assert_eq!(by_col1, vec![1, 2, 0]);
    }

    #[test]
    fn reorder_without_sort_restores_file_order() {
        let rows = make_rows(&[&["a"], &["b"], &["c"], &["d"]]);
        assert_eq!(reorder_indices(&rows, &[3, 0, 2], None), vec![0, 2, 3]);
    }

    // --- sort_indices ---

    #[test]
    fn sort_string_ascending() {
        let rows = make_rows(&[&["Charlie"], &["Alice"], &["Bob"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_string_descending() {
        let rows = make_rows(&[&["Charlie"], &["Alice"], &["Bob"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]);
    }

    #[test]
    fn sort_numeric_ascending() {
        let rows = make_rows(&[&["100"], &["3"], &["25"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_numeric_descending() {
        let rows = make_rows(&[&["100"], &["3"], &["25"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]);
    }

    #[test]
    fn sort_respects_filtered_indices() {
        let rows = make_rows(&[&["C"], &["A"], &["B"], &["D"]]);
        let indices = vec![0, 2, 3];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![2, 0, 3]);
    }

    #[test]
    fn sort_mixed_numeric_and_string_uses_string_comparison() {
        let rows = make_rows(&[&["banana"], &["10"], &["apple"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_empty_indices() {
        let rows = make_rows(&[&["A"]]);
        let result = sort_indices(&rows, &[], 0, false, SortDirection::Ascending);
        assert!(result.is_empty());
    }

    #[test]
    fn sort_with_missing_column() {
        let rows = make_rows(&[&["A", "1"], &["B"]]);
        let indices = vec![0, 1];
        let result = sort_indices(&rows, &indices, 1, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 0]);
    }

    #[test]
    fn sort_numeric_with_nan() {
        let rows = make_rows(&[&["NaN"], &["0"], &["1"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_numeric_with_nan_descending() {
        let rows = make_rows(&[&["NaN"], &["0"], &["1"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Descending);
        assert_eq!(result, vec![0, 2, 1]);
    }

    // --- compute_numeric_columns ---

    #[test]
    fn numeric_columns_detection() {
        let data = make_csv_data(
            &["name", "age", "score"],
            &[&["Alice", "30", "95.5"], &["Bob", "25", "87.0"]],
        );
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![false, true, true]);
    }

    #[test]
    fn numeric_columns_with_empty_values() {
        let data = make_csv_data(&["val"], &[&["1"], &[""], &["3"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![true]);
    }

    #[test]
    fn numeric_columns_majority_numeric() {
        // 2/3 numeric → treated as numeric
        let data = make_csv_data(&["col"], &[&["1"], &["abc"], &["3"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![true]);
    }

    #[test]
    fn numeric_columns_majority_text() {
        // 1/3 numeric → not numeric
        let data = make_csv_data(&["col"], &[&["abc"], &["def"], &["3"]]);
        let result = compute_numeric_columns(&data.rows, data.headers.len());
        assert_eq!(result, vec![false]);
    }

    // --- extract_column_values ---

    #[test]
    fn extract_values_basic() {
        let rows = make_rows(&[&["10", "a"], &["20", "b"], &["30", "c"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (1, 20.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_skips_non_numeric() {
        let rows = make_rows(&[&["10"], &["abc"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_respects_indices() {
        let rows = make_rows(&[&["10"], &["20"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 2], 0);
        assert_eq!(result, vec![(0, 10.0), (2, 30.0)]);
    }

    #[test]
    fn extract_values_empty_indices() {
        let rows = make_rows(&[&["10"]]);
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

    // --- chart axes ---

    fn tick_labels(axis: &crate::data::ChartAxis) -> Vec<&str> {
        axis.ticks.iter().map(|(_, label)| label.as_str()).collect()
    }

    #[test]
    fn format_tick_uses_digits_for_the_tick_spacing() {
        assert_eq!(format_tick(100.0, 200.0), "100");
        assert_eq!(format_tick(1.5, 3.0), "1.5");
        assert_eq!(format_tick(0.5, 1.0), "0.50");
    }

    #[test]
    fn format_tick_has_no_negative_zero() {
        assert_eq!(format_tick(-0.3, 200.0), "0");
    }

    #[test]
    fn format_tick_uses_exponent_for_large_values() {
        assert_eq!(format_tick(2e12, 1e12), "2.000e12");
    }

    #[test]
    fn format_tick_tells_close_values_apart() {
        assert_eq!(format_tick(1e12 + 2.0, 2.0), "1.00000000000200e12");
        assert_eq!(format_tick(1.000000005, 1e-8), "1.0000000050");
    }

    #[test]
    fn points_axes_includes_zero_for_bars() {
        let axes = points_axes(&[1.0, 3.0], true);
        assert_eq!((axes.y.lo, axes.y.hi), (0.0, 3.0));
        assert_eq!(axes.x, None);
    }

    #[test]
    fn points_axes_uses_value_range_for_lines() {
        let axes = points_axes(&[1.0, 3.0], false);
        assert_eq!((axes.y.lo, axes.y.hi), (1.0, 3.0));
        assert_eq!(tick_labels(&axes.y), ["1.0", "2.0", "3.0"]);
    }

    #[test]
    fn points_axes_widens_equal_values() {
        let axes = points_axes(&[5.0, 5.0], false);
        assert_eq!((axes.y.lo, axes.y.hi), (5.0, 6.0));
    }

    #[test]
    fn points_axes_widens_large_equal_values() {
        let axes = points_axes(&[1e16, 1e16], false);
        assert!(axes.y.hi > axes.y.lo);
    }

    #[test]
    fn points_axes_keeps_negative_range_for_bars() {
        let axes = points_axes(&[-3.0, -1.0], true);
        assert_eq!((axes.y.lo, axes.y.hi), (-3.0, -1.0));
    }

    #[test]
    fn points_axes_without_values_has_no_ticks() {
        assert!(points_axes(&[], false).y.ticks.is_empty());
    }

    #[test]
    fn pairs_axes_uses_x_and_y_ranges() {
        let axes = pairs_axes(&[(0.0, 10.0), (2.0, 20.0)]);
        let x = axes.x.expect("scatter has an x axis");
        assert_eq!((x.lo, x.hi), (0.0, 2.0));
        assert_eq!((axes.y.lo, axes.y.hi), (10.0, 20.0));
    }

    #[test]
    fn histogram_axes_labels_even_max_count() {
        let axes = histogram_axes(&[0.0, 10.0], &[2, 0]);
        assert_eq!((axes.y.lo, axes.y.hi), (0.0, 2.0));
        assert_eq!(tick_labels(&axes.y), ["0", "1", "2"]);
        let x = axes.x.expect("histogram has an x axis");
        assert_eq!((x.lo, x.hi), (0.0, 10.0));
    }

    #[test]
    fn histogram_axes_skips_middle_tick_for_odd_max_count() {
        let axes = histogram_axes(&[0.0, 10.0], &[3, 0]);
        assert_eq!(tick_labels(&axes.y), ["0", "3"]);
    }

    #[test]
    fn histogram_axes_centers_equal_values() {
        let x = histogram_axes(&[4.0, 4.0], &[0, 2])
            .x
            .expect("histogram has an x axis");
        assert_eq!((x.lo, x.hi), (3.5, 4.5));
    }

    #[test]
    fn histogram_axes_widens_large_equal_values() {
        let x = histogram_axes(&[1e16, 1e16], &[0, 2])
            .x
            .expect("histogram has an x axis");
        assert!(x.hi > x.lo);
    }

    #[test]
    fn histogram_axes_without_values_has_no_ticks() {
        let axes = histogram_axes(&[], &[0, 0]);
        assert!(axes.y.ticks.is_empty());
        assert!(axes.x.expect("histogram has an x axis").ticks.is_empty());
    }

    #[test]
    fn chart_legend_uses_single_line_names() {
        assert_eq!(chart_legend(ChartType::Line, "A\nB", "C"), "A B");
        assert_eq!(
            chart_legend(ChartType::Scatter, "A", "B\nC"),
            "Y: A  X: B C"
        );
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
        let rows = make_rows(&[&["10"], &["NaN"], &["inf"], &["-inf"], &["30"]]);
        let result = extract_column_values(&rows, &[0, 1, 2, 3, 4], 0);
        assert_eq!(result, vec![(0, 10.0), (4, 30.0)]);
    }

    #[test]
    fn extract_values_out_of_bounds_index() {
        let rows = make_rows(&[&["10"], &["20"]]);
        let result = extract_column_values(&rows, &[0, 5, 1], 0);
        assert_eq!(result, vec![(0, 10.0), (1, 20.0)]);
    }

    #[test]
    fn extract_values_out_of_bounds_col() {
        let rows = make_rows(&[&["10"], &["20"]]);
        let result = extract_column_values(&rows, &[0, 1], 5);
        assert!(result.is_empty());
    }

    #[test]
    fn extract_values_empty_string() {
        let rows = make_rows(&[&["10"], &[""], &["30"]]);
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
        assert!(bins[0] >= 3);
        assert_eq!(*bins.last().unwrap(), 1);
    }

    #[test]
    fn extract_values_negative_numbers() {
        let rows = make_rows(&[&["-3.14"], &["2.5"], &["-0.001"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, -3.14), (1, 2.5), (2, -0.001)]);
    }

    // --- extract_scatter_pairs ---

    #[test]
    fn scatter_pairs_basic() {
        let rows = make_rows(&[&["1", "10"], &["2", "20"], &["3", "30"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2], 0, 1);
        assert_eq!(result, vec![(1.0, 10.0), (2.0, 20.0), (3.0, 30.0)]);
    }

    #[test]
    fn scatter_pairs_skips_non_numeric_in_either_column() {
        let rows = make_rows(&[&["1", "10"], &["abc", "20"], &["3", "xyz"], &["4", "40"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2, 3], 0, 1);
        assert_eq!(result, vec![(1.0, 10.0), (4.0, 40.0)]);
    }

    #[test]
    fn scatter_pairs_empty_indices() {
        let rows = make_rows(&[&["1", "10"]]);
        let result = extract_scatter_pairs(&rows, &[], 0, 1);
        assert!(result.is_empty());
    }

    #[test]
    fn scatter_pairs_filters_nan_infinity() {
        let rows = make_rows(&[&["1", "NaN"], &["inf", "2"], &["3", "4"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2], 0, 1);
        assert_eq!(result, vec![(3.0, 4.0)]);
    }

    // --- compute_column_stats ---

    #[test]
    fn column_stats_basic() {
        let rows = make_rows(&[&["10"], &["20"], &["30"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2], 0).unwrap();
        assert_eq!(stats.count, 3);
        assert!((stats.sum - 60.0).abs() < f64::EPSILON);
        assert!((stats.min - 10.0).abs() < f64::EPSILON);
        assert!((stats.max - 30.0).abs() < f64::EPSILON);
        assert!((stats.mean - 20.0).abs() < f64::EPSILON);
        assert!((stats.median - 20.0).abs() < f64::EPSILON);
        // Sample stddev of [10, 20, 30]: sqrt(((10-20)^2 + (20-20)^2 + (30-20)^2) / 2) = sqrt(100) = 10
        assert!((stats.stddev - 10.0).abs() < 1e-10);
    }

    #[test]
    fn column_stats_single_value() {
        let rows = make_rows(&[&["42"]]);
        let stats = compute_column_stats(&rows, &[0], 0).unwrap();
        assert_eq!(stats.count, 1);
        assert!((stats.sum - 42.0).abs() < f64::EPSILON);
        assert!((stats.min - 42.0).abs() < f64::EPSILON);
        assert!((stats.max - 42.0).abs() < f64::EPSILON);
        assert!((stats.mean - 42.0).abs() < f64::EPSILON);
        assert!((stats.median - 42.0).abs() < f64::EPSILON);
        assert!((stats.stddev - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn column_stats_skips_non_numeric() {
        let rows = make_rows(&[&["10"], &["abc"], &["30"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2], 0).unwrap();
        assert_eq!(stats.count, 2);
        assert!((stats.sum - 40.0).abs() < f64::EPSILON);
    }

    #[test]
    fn column_stats_no_numeric_values() {
        let rows = make_rows(&[&["abc"], &["def"]]);
        assert!(compute_column_stats(&rows, &[0, 1], 0).is_none());
    }

    #[test]
    fn column_stats_empty_indices() {
        let rows = make_rows(&[&["10"]]);
        assert!(compute_column_stats(&rows, &[], 0).is_none());
    }

    #[test]
    fn column_stats_respects_filtered_indices() {
        let rows = make_rows(&[&["10"], &["20"], &["30"], &["40"]]);
        let stats = compute_column_stats(&rows, &[1, 3], 0).unwrap();
        assert_eq!(stats.count, 2);
        assert!((stats.sum - 60.0).abs() < f64::EPSILON);
        assert!((stats.min - 20.0).abs() < f64::EPSILON);
        assert!((stats.max - 40.0).abs() < f64::EPSILON);
        assert!((stats.median - 30.0).abs() < f64::EPSILON); // (20+40)/2
    }

    #[test]
    fn column_stats_median_odd() {
        let rows = make_rows(&[&["5"], &["1"], &["3"], &["4"], &["2"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2, 3, 4], 0).unwrap();
        assert!((stats.median - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn column_stats_median_even() {
        let rows = make_rows(&[&["4"], &["1"], &["3"], &["2"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2, 3], 0).unwrap();
        assert!((stats.median - 2.5).abs() < f64::EPSILON); // (2+3)/2
    }

    // --- filter_columns_by_regex ---

    #[test]
    fn filter_columns_by_regex_empty_pattern() {
        let headers = vec!["Name".into(), "Age".into(), "City".into()];
        let result = filter_columns_by_regex(&headers, "").unwrap();
        assert_eq!(result, vec![0, 1, 2]);
    }

    #[test]
    fn filter_columns_by_regex_partial_match() {
        let headers = vec!["Name".into(), "Age".into(), "City".into()];
        let result = filter_columns_by_regex(&headers, "a").unwrap();
        assert_eq!(result, vec![0, 1]); // Name, Age
    }

    #[test]
    fn filter_columns_by_regex_case_insensitive() {
        let headers = vec!["Name".into(), "age".into(), "CITY".into()];
        let result = filter_columns_by_regex(&headers, "AGE|city").unwrap();
        assert_eq!(result, vec![1, 2]);
    }

    #[test]
    fn filter_columns_by_regex_no_match() {
        let headers = vec!["Name".into(), "Age".into()];
        let result = filter_columns_by_regex(&headers, "zzz").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn filter_columns_by_regex_invalid_pattern() {
        let headers = vec!["Name".into()];
        assert!(filter_columns_by_regex(&headers, "[invalid").is_err());
    }

    // --- parse_column_filter ---

    #[test]
    fn parse_column_filter_global() {
        let headers = vec!["Name".into(), "City".into()];
        let (col, pattern) = parse_column_filter("tokyo", &headers);
        assert_eq!(col, None);
        assert_eq!(pattern, "tokyo");
    }

    #[test]
    fn parse_column_filter_with_col() {
        let headers = vec!["Name".into(), "City".into()];
        let (col, pattern) = parse_column_filter("city:tokyo", &headers);
        assert_eq!(col, Some(1));
        assert_eq!(pattern, "tokyo");
    }

    #[test]
    fn parse_column_filter_unknown_col() {
        let headers = vec!["Name".into(), "City".into()];
        let (col, pattern) = parse_column_filter("unknown:x", &headers);
        assert_eq!(col, None);
        assert_eq!(pattern, "unknown:x");
    }

    // --- anchored_position / RowPositions ---

    fn anchor(old: &[usize], new: &[usize], top: usize) -> Option<usize> {
        let row_count = old.iter().chain(new).max().map_or(0, |&max| max + 1);
        anchored_position(old, &RowPositions::new(new, row_count), top)
    }

    #[test]
    fn row_positions_looks_up_shown_rows() {
        let positions = RowPositions::new(&[4, 0, 2], 5);
        assert_eq!(positions.get(4), Some(0));
        assert_eq!(positions.get(2), Some(2));
        assert_eq!(positions.get(1), None);
        assert_eq!(positions.get(9), None);
    }

    #[test]
    fn anchored_position_keeps_unchanged_rows() {
        assert_eq!(anchor(&[0, 1, 2, 3, 4], &[0, 1, 2, 3, 4], 2), Some(2));
    }

    #[test]
    fn anchored_position_moves_to_next_remaining_row() {
        assert_eq!(anchor(&[0, 1, 2, 3, 4], &[1, 3, 4], 2), Some(1));
    }

    #[test]
    fn anchored_position_falls_back_to_last_row() {
        assert_eq!(anchor(&[0, 1, 2, 3, 4], &[0, 1], 3), Some(1));
    }

    #[test]
    fn anchored_position_follows_row_when_widening() {
        assert_eq!(anchor(&[1, 3], &[0, 1, 2, 3, 4], 1), Some(3));
    }

    #[test]
    fn anchored_position_uses_sorted_order() {
        assert_eq!(anchor(&[4, 3, 2, 1, 0], &[0, 2, 4], 1), Some(1));
    }

    #[test]
    fn anchored_position_without_rows_to_keep() {
        assert_eq!(anchor(&[0, 1], &[], 0), None);
        assert_eq!(anchor(&[], &[0, 1], 0), None);
    }

    #[test]
    fn anchored_position_with_out_of_range_top() {
        assert_eq!(anchor(&[0, 1], &[0, 1], 9), Some(1));
    }

    // --- RowFilter: `&` regex ---

    #[test]
    fn filter_rows_regex_basic() {
        let rows = make_rows(&[&["Tokyo"], &["Osaka"], &["Kyoto"]]);
        // Tokyo, Kyoto
        assert_eq!(filter_all(&rows, "", "to", None), (vec![0, 2], false));
    }

    #[test]
    fn filter_rows_regex_column_specific() {
        let rows = make_rows(&[&["Alice", "Tokyo"], &["Bob", "Osaka"], &["Carol", "Tokyo"]]);
        assert_eq!(filter_all(&rows, "", "tokyo", Some(1)), (vec![0, 2], false));
    }

    #[test]
    fn filter_rows_regex_column_missing_in_short_row() {
        let rows = make_rows(&[&["Tokyo"], &["Bob", "Tokyo"]]);
        assert_eq!(filter_all(&rows, "", "tokyo", Some(1)), (vec![1], false));
    }

    #[test]
    fn filter_rows_regex_empty_pattern() {
        let rows = make_rows(&[&["a"], &["b"]]);
        assert_eq!(filter_all(&rows, "", "", None), (vec![0, 1], false));
    }

    #[test]
    fn filter_rows_regex_invalid_pattern() {
        let rows = make_rows(&[&["a"]]);
        assert_eq!(filter_all(&rows, "", "[bad", None), (vec![0], true));
    }

    #[test]
    fn filter_rows_regex_out_of_range_col() {
        // target_col=Some(99) is beyond any row's columns — should match nothing
        let rows = make_rows(&[&["Tokyo"], &["Osaka"]]);
        assert_eq!(filter_all(&rows, "", "Tokyo", Some(99)), (vec![], false));
    }

    // --- copied_status_message ---

    #[test]
    fn copied_status_message_singular() {
        assert_eq!(copied_status_message("JSON", 1), "Copied as JSON (1 row)");
    }

    #[test]
    fn copied_status_message_plural() {
        assert_eq!(
            copied_status_message("Markdown", 2),
            "Copied as Markdown (2 rows)"
        );
    }

    // --- export_json ---

    #[test]
    fn export_json_basic() {
        let rows = make_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        let result = export_json(&["name".into(), "age".into()], &rows, &[0, 1], &[0, 1]);
        assert_eq!(
            result,
            "[\n  {\"name\": \"Alice\", \"age\": \"30\"},\n  {\"name\": \"Bob\", \"age\": \"25\"}\n]\n"
        );
    }

    #[test]
    fn export_json_special_chars() {
        let rows = make_rows(&[&["say \"hi\"", "line1\nline2"]]);
        let result = export_json(&["msg".into(), "note".into()], &rows, &[0], &[0, 1]);
        assert!(result.contains("say \\\"hi\\\""));
        assert!(result.contains("line1\\nline2"));
    }

    #[test]
    fn export_json_empty_rows() {
        let rows = make_rows(&[&["a"]]);
        let result = export_json(&["col".into()], &rows, &[], &[0]);
        assert_eq!(result, "[]\n");
    }

    #[test]
    fn export_json_visible_cols() {
        let rows = make_rows(&[&["Alice", "30", "Tokyo"]]);
        let result = export_json(
            &["name".into(), "age".into(), "city".into()],
            &rows,
            &[0],
            &[0, 2],
        );
        assert!(result.contains("\"name\""));
        assert!(result.contains("\"city\""));
        assert!(!result.contains("\"age\""));
    }

    // --- export_markdown ---

    #[test]
    fn export_markdown_basic() {
        let rows = make_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        let result = export_markdown(&["name".into(), "age".into()], &rows, &[0, 1], &[0, 1]);
        assert_eq!(
            result,
            "| name | age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n"
        );
    }

    #[test]
    fn export_markdown_pipe_escape() {
        let rows = make_rows(&[&["a|b"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("a\\|b"));
    }

    #[test]
    fn export_markdown_empty_rows() {
        let rows = make_rows(&[&["a"]]);
        let result = export_markdown(&["col".into()], &rows, &[], &[0]);
        assert_eq!(result, "| col |\n| --- |\n");
    }

    #[test]
    fn export_markdown_visible_cols() {
        let rows = make_rows(&[&["Alice", "30", "Tokyo"]]);
        let result = export_markdown(
            &["name".into(), "age".into(), "city".into()],
            &rows,
            &[0],
            &[0, 2],
        );
        assert!(result.contains("| name | city |"));
        assert!(!result.contains("age"));
    }

    #[test]
    fn export_json_control_chars() {
        let rows = make_rows(&[&["a\x00b\x08c\x1f"]]);
        let result = export_json(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("a\\u0000b\\u0008c\\u001f"));
    }

    #[test]
    fn export_markdown_newline_in_cell() {
        let rows = make_rows(&[&["line1\nline2"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("| line1 line2 |"));
    }

    #[test]
    fn export_json_out_of_bounds_index() {
        let rows = make_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[0, 99], &[0]);
        // Row index 99 is skipped safely; only Alice appears
        assert_eq!(result, "[\n  {\"name\": \"Alice\"}\n]\n");
    }

    #[test]
    fn export_json_header_escape() {
        let rows = make_rows(&[&["val"]]);
        let result = export_json(&["col\"name".into()], &rows, &[0], &[0]);
        assert!(result.contains("\"col\\\"name\""));
    }

    #[test]
    fn export_markdown_header_pipe_escape() {
        let rows = make_rows(&[&["val"]]);
        let result = export_markdown(&["col|name".into()], &rows, &[0], &[0]);
        assert!(result.contains("col\\|name"));
    }

    #[test]
    fn export_json_empty_visible_cols() {
        let rows = make_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[0], &[]);
        assert_eq!(result, "[]\n");
    }

    #[test]
    fn export_markdown_empty_visible_cols() {
        let rows = make_rows(&[&["Alice"]]);
        let result = export_markdown(&["name".into()], &rows, &[0], &[]);
        assert_eq!(result, "");
    }

    #[test]
    fn export_json_out_of_bounds_first() {
        // When the first filtered index is out of bounds, the written counter
        // prevents a leading comma from appearing in the output.
        let rows = make_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[99, 0], &[0]);
        assert_eq!(result, "[\n  {\"name\": \"Alice\"}\n]\n");
    }

    #[test]
    fn export_markdown_cr_in_cell() {
        let rows = make_rows(&[&["line1\r\nline2"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("| line1  line2 |"));
    }

    #[test]
    fn export_json_duplicate_headers() {
        let rows = make_rows(&[&["Alice", "Bob", "30"]]);
        let result = export_json(
            &["name".into(), "name".into(), "value".into()],
            &rows,
            &[0],
            &[0, 1, 2],
        );
        assert_eq!(
            result,
            "[\n  {\"name\": \"Alice\", \"name_2\": \"Bob\", \"value\": \"30\"}\n]\n"
        );
    }

    #[test]
    fn export_json_triple_duplicate_headers() {
        let rows = make_rows(&[&["a", "b", "c"]]);
        let result = export_json(
            &["x".into(), "x".into(), "x".into()],
            &rows,
            &[0],
            &[0, 1, 2],
        );
        assert_eq!(
            result,
            "[\n  {\"x\": \"a\", \"x_2\": \"b\", \"x_3\": \"c\"}\n]\n"
        );
    }

    #[test]
    fn export_json_empty_duplicate_headers() {
        let rows = make_rows(&[&["a", "b"]]);
        let result = export_json(&["".into(), "".into()], &rows, &[0], &[0, 1]);
        assert_eq!(result, "[\n  {\"\": \"a\", \"_2\": \"b\"}\n]\n");
    }

    #[test]
    fn deduplicate_json_keys_basic() {
        let headers = vec!["a".into(), "b".into(), "a".into(), "c".into(), "a".into()];
        let keys = deduplicate_json_keys(&headers, &[0, 1, 2, 3, 4]);
        assert_eq!(keys, vec!["a", "b", "a_2", "c", "a_3"]);
    }

    #[test]
    fn deduplicate_json_keys_collision_with_existing() {
        // "a_2" already exists as a header, so the suffix for the second "a" must skip to _3.
        let headers: Vec<String> = vec!["a".into(), "a_2".into(), "a".into()];
        let keys = deduplicate_json_keys(&headers, &[0, 1, 2]);
        assert_eq!(keys, vec!["a", "a_2", "a_3"]);
    }

    // --- caret_scroll_offset ---

    #[test]
    fn caret_scroll_offset_is_zero_while_the_caret_fits() {
        assert_eq!(caret_scroll_offset(50.0, 1.0, 100.0), 0.0);
    }

    #[test]
    fn caret_scroll_offset_is_zero_at_the_right_edge() {
        assert_eq!(caret_scroll_offset(99.0, 1.0, 100.0), 0.0);
    }

    #[test]
    fn caret_scroll_offset_puts_an_overflowing_caret_at_the_right_edge() {
        assert_eq!(caret_scroll_offset(150.0, 1.0, 100.0), 51.0);
    }

    #[test]
    fn caret_scroll_offset_with_no_width_keeps_the_caret_at_the_left_edge() {
        assert_eq!(caret_scroll_offset(30.0, 1.0, 0.0), 31.0);
    }

    // --- TextInputState ---

    #[test]
    fn text_input_new_puts_the_caret_at_the_end() {
        let state = TextInputState::new("東京");
        assert_eq!(state.selected_range_utf16(), 2..2);
        assert_eq!(state.marked_range_utf16(), None);
        assert!(!state.is_composing());
    }

    #[test]
    fn text_input_replace_appends_at_the_caret() {
        let mut state = TextInputState::new("ab");
        state.replace(None, "c");
        assert_eq!(state.text(), "abc");
        assert_eq!(state.selected_range_utf16(), 3..3);
    }

    #[test]
    fn text_input_composition_is_updated_then_committed() {
        let mut state = TextInputState::new("a");
        state.replace_and_mark(None, "k", None);
        assert_eq!(state.text(), "ak");
        assert_eq!(state.marked_range_utf16(), Some(1..2));
        state.replace_and_mark(None, "か", None);
        assert_eq!(state.text(), "aか");
        assert_eq!(state.marked_range_utf16(), Some(1..2));
        state.replace(None, "科");
        assert_eq!(state.text(), "a科");
        assert_eq!(state.marked_range_utf16(), None);
        assert_eq!(state.selected_range_utf16(), 2..2);
        assert!(!state.is_composition_open());
    }

    #[test]
    fn text_input_empty_composition_stays_open_until_unmark() {
        let mut state = TextInputState::new("a");
        state.replace_and_mark(None, "か", None);
        state.replace_and_mark(None, "", None);
        assert_eq!(state.text(), "a");
        assert!(!state.is_composing());
        assert!(state.is_composition_open());
        state.unmark();
        assert!(!state.is_composing());
        assert!(!state.is_composition_open());
    }

    #[test]
    fn text_input_unmark_keeps_the_text_and_the_selection() {
        let mut state = TextInputState::new("");
        state.replace_and_mark(None, "とうきょう", Some(0..2));
        state.unmark();
        assert_eq!(state.text(), "とうきょう");
        assert_eq!(state.selected_range_utf16(), 0..2);
    }

    #[test]
    fn text_input_replace_in_the_middle_keeps_the_text_after_the_range() {
        let mut state = TextInputState::new("abcXYZ");
        state.replace(Some(1..2), "Q");
        assert_eq!(state.text(), "aQcXYZ");
        assert_eq!(state.selected_range_utf16(), 2..2);
    }

    #[test]
    fn text_input_replace_part_of_the_marked_text() {
        let mut state = TextInputState::new("");
        state.replace_and_mark(None, "かんじ", None);
        state.replace_and_mark(Some(1..2), "ン", None);
        assert_eq!(state.text(), "かンじ");
        assert_eq!(state.marked_range_utf16(), Some(1..2));
    }

    #[test]
    fn text_input_reconversion_marks_committed_text_without_changing_it() {
        let mut state = TextInputState::new("東京");
        state.replace_and_mark(Some(0..2), "東京", None);
        assert_eq!(state.text(), "東京");
        assert_eq!(state.marked_range_utf16(), Some(0..2));
    }

    #[test]
    fn text_input_selection_inside_the_marked_text_is_relative() {
        let mut state = TextInputState::new("x");
        state.replace_and_mark(None, "とうきょう", Some(2..5));
        assert_eq!(state.selected_range_utf16(), 3..6);
    }

    #[test]
    fn text_input_selection_inside_the_marked_text_is_fitted_to_it() {
        let mut state = TextInputState::new("x");
        state.replace_and_mark(None, "かな", Some(1..10));
        assert_eq!(state.selected_range_utf16(), 2..3);
    }

    #[test]
    fn text_input_ranges_past_the_end_are_fitted() {
        let mut state = TextInputState::new("ab");
        assert_eq!(state.text_for_range_utf16(1..10), ("b".to_string(), 1..2));
        state.replace(Some(5..9), "c");
        assert_eq!(state.text(), "abc");
    }

    #[test]
    // The platform can hand over a reversed range, which is what this checks.
    #[allow(clippy::reversed_empty_ranges)]
    fn text_input_reversed_ranges_are_ordered() {
        let state = TextInputState::new("abc");
        assert_eq!(state.text_for_range_utf16(2..1), ("b".to_string(), 1..2));
    }

    #[test]
    fn text_input_offsets_inside_a_surrogate_pair_do_not_split_it() {
        let mut state = TextInputState::new("a😀b");
        assert_eq!(state.len_utf16(), 4);
        // UTF-16 offset 2 is between the two halves of the emoji.
        assert_eq!(state.text_for_range_utf16(2..4), ("😀b".to_string(), 1..4));
        state.set_selected_range_utf16(2..2);
        assert_eq!(state.selected_range_utf16(), 1..1);
    }

    #[test]
    fn text_input_japanese_utf16_offsets() {
        let state = TextInputState::new("部署:営業");
        assert_eq!(state.len_utf16(), 5);
        assert_eq!(state.text_for_range_utf16(3..5), ("営業".to_string(), 3..5));
    }

    #[test]
    fn text_input_delete_backward_removes_the_char_before_the_caret() {
        let mut state = TextInputState::new("営業");
        state.delete_backward();
        assert_eq!(state.text(), "営");
        assert_eq!(state.selected_range_utf16(), 1..1);
    }

    #[test]
    fn text_input_delete_backward_removes_the_selection() {
        let mut state = TextInputState::new("abcd");
        state.set_selected_range_utf16(1..3);
        state.delete_backward();
        assert_eq!(state.text(), "ad");
        assert_eq!(state.selected_range_utf16(), 1..1);
    }

    #[test]
    fn text_input_delete_backward_ends_an_empty_composition() {
        let mut state = TextInputState::new("ab");
        state.replace_and_mark(None, "", None);
        assert!(state.is_composition_open());
        state.delete_backward();
        assert_eq!(state.text(), "a");
        assert!(!state.is_composition_open());
    }

    #[test]
    fn text_input_delete_backward_on_empty_text_does_nothing() {
        let mut state = TextInputState::new("");
        state.delete_backward();
        assert_eq!(state.text(), "");
        assert_eq!(state.selected_range_utf16(), 0..0);
    }

    #[test]
    fn text_input_select_all_then_replace() {
        let mut state = TextInputState::new("abc");
        state.select_all();
        assert_eq!(state.selected_range_utf16(), 0..3);
        state.replace(None, "x");
        assert_eq!(state.text(), "x");
    }

    #[test]
    fn text_input_select_all_is_ignored_while_composing() {
        let mut state = TextInputState::new("a");
        state.replace_and_mark(None, "か", None);
        state.select_all();
        assert_eq!(state.selected_range_utf16(), 2..2);
    }

    #[test]
    fn text_input_newlines_become_spaces() {
        let mut state = TextInputState::new("");
        state.replace(None, "a\r\nb");
        assert_eq!(state.text(), "a  b");
        state.replace_and_mark(None, "c\nd", None);
        assert_eq!(state.text(), "a  bc d");
        assert_eq!(state.marked_range_utf16(), Some(4..7));
    }
}
