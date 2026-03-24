use std::rc::Rc;

use regex::RegexBuilder;

use crate::data::{CsvData, SortDirection};

/// Pixel width per character (monospace approximation at text_sm)
pub(crate) const CHAR_WIDTH: f32 = 7.5;
pub(crate) const MIN_COL_WIDTH: f32 = 50.0;
pub(crate) const MAX_COL_WIDTH: f32 = 400.0;
pub(crate) const COL_PADDING: f32 = 24.0;
pub(crate) const ROW_NUM_WIDTH: f32 = 16.0;

pub(crate) fn compute_column_widths(data: &CsvData) -> Vec<f32> {
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
            (max_len as f32 * CHAR_WIDTH + COL_PADDING).clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
        })
        .collect()
}

pub(crate) fn row_number_col_width(total_rows: usize) -> f32 {
    let digits = total_rows.max(1).ilog10() as usize + 1;
    (digits as f32 * CHAR_WIDTH + ROW_NUM_WIDTH).max(40.0)
}

pub(crate) fn filter_rows(rows: &[Rc<Vec<String>>], query: &str) -> Vec<usize> {
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
pub(crate) fn compute_numeric_columns(rows: &[Vec<String>], col_count: usize) -> Vec<bool> {
    (0..col_count)
        .map(|col| {
            let mut numeric_count: usize = 0;
            let mut non_empty_count: usize = 0;
            for row in rows {
                let val = row.get(col).map(|s| s.as_str()).unwrap_or("");
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

pub(crate) fn sort_indices(
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
pub(crate) fn extract_column_values(
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
pub(crate) fn downsample<T: Copy>(values: &[T], max: usize) -> Vec<T> {
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
pub(crate) fn extract_scatter_pairs(
    rows: &[Rc<Vec<String>>],
    indices: &[usize],
    x_col: usize,
    y_col: usize,
) -> Vec<(f64, f64)> {
    indices
        .iter()
        .filter_map(|&i| {
            let row = rows.get(i)?;
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
pub(crate) struct ColumnStats {
    pub(crate) count: usize,
    pub(crate) sum: f64,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) mean: f64,
    pub(crate) median: f64,
    pub(crate) stddev: f64,
}

/// Compute summary statistics for a numeric column over the given row indices.
/// Collects values into a Vec for median (requires sorting) and stddev (two-pass).
/// Returns `None` if no finite numeric values exist.
pub(crate) fn compute_column_stats(
    rows: &[Rc<Vec<String>>],
    indices: &[usize],
    col: usize,
) -> Option<ColumnStats> {
    let mut values: Vec<f64> = Vec::new();
    let mut sum: f64 = 0.0;
    let mut min: f64 = f64::INFINITY;
    let mut max: f64 = f64::NEG_INFINITY;

    for &row_idx in indices {
        let v = rows
            .get(row_idx)
            .and_then(|row| row.get(col))
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
pub(crate) fn filter_columns_by_regex(
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
pub(crate) fn parse_column_filter(query: &str, headers: &[String]) -> (Option<usize>, String) {
    if let Some((prefix, suffix)) = query.split_once(':')
        && let Some(idx) = headers.iter().position(|h| h.eq_ignore_ascii_case(prefix))
    {
        return (Some(idx), suffix.to_string());
    }
    (None, query.to_string())
}

/// Filter rows by regex, optionally targeting a specific column.
/// Applies to the given subset of row indices.
pub(crate) fn filter_rows_regex(
    rows: &[Rc<Vec<String>>],
    indices: &[usize],
    pattern: &str,
    target_col: Option<usize>,
) -> Result<Vec<usize>, regex::Error> {
    if pattern.is_empty() {
        return Ok(indices.to_vec());
    }
    let re = RegexBuilder::new(pattern).case_insensitive(true).build()?;
    Ok(indices
        .iter()
        .copied()
        .filter(|&i| {
            let Some(row) = rows.get(i) else { return false };
            match target_col {
                Some(col) => row.get(col).is_some_and(|cell| re.is_match(cell)),
                None => row.iter().any(|cell| re.is_match(cell)),
            }
        })
        .collect())
}

/// Compute histogram bin counts for the given values.
pub(crate) fn compute_histogram_bins(values: &[f64], bin_count: usize) -> Vec<usize> {
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

/// Export visible data as JSON (array of objects).
pub(crate) fn export_json(
    headers: &[String],
    rows: &[Rc<Vec<String>>],
    filtered_indices: &[usize],
    visible_col_indices: &[usize],
) -> String {
    if visible_col_indices.is_empty() {
        return "[]\n".to_string();
    }
    let mut out = String::new();
    let mut written = 0usize;
    for &row_idx in filtered_indices {
        let Some(row) = rows.get(row_idx) else {
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
            let key = headers.get(col_idx).map_or("", |s| s.as_str());
            let val = row.get(col_idx).map_or("", |s| s.as_str());
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
pub(crate) fn export_markdown(
    headers: &[String],
    rows: &[Rc<Vec<String>>],
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
        let Some(row) = rows.get(row_idx) else {
            continue;
        };
        out.push('|');
        for &col_idx in visible_col_indices {
            let val = row.get(col_idx).map_or("", |s| s.as_str());
            out.push_str(&format!(" {} |", escape_markdown_cell(val)));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::CsvData;

    fn make_csv_data(headers: &[&str], rows: &[&[&str]]) -> CsvData {
        CsvData {
            headers: headers.iter().map(|s| s.to_string()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|s| s.to_string()).collect())
                .collect(),
            metadata: Vec::new(),
        }
    }

    fn make_rc_rows(rows: &[&[&str]]) -> Vec<Rc<Vec<String>>> {
        rows.iter()
            .map(|row| Rc::new(row.iter().map(|s| s.to_string()).collect()))
            .collect()
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
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.rows[0], vec!["1", "2"]);
        assert_eq!(data.rows[1], vec!["4", "5", "6"]);
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
        let indices = vec![0, 2, 3];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![2, 0, 3]);
    }

    #[test]
    fn sort_mixed_numeric_and_string_uses_string_comparison() {
        let rows = make_rc_rows(&[&["banana"], &["10"], &["apple"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, false, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
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
        let result = sort_indices(&rows, &indices, 1, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 0]);
    }

    #[test]
    fn sort_numeric_with_nan() {
        let rows = make_rc_rows(&[&["NaN"], &["0"], &["1"]]);
        let indices = vec![0, 1, 2];
        let result = sort_indices(&rows, &indices, 0, true, SortDirection::Ascending);
        assert_eq!(result, vec![1, 2, 0]);
    }

    #[test]
    fn sort_numeric_with_nan_descending() {
        let rows = make_rc_rows(&[&["NaN"], &["0"], &["1"]]);
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
        assert!(bins[0] >= 3);
        assert_eq!(*bins.last().unwrap(), 1);
    }

    #[test]
    fn extract_values_negative_numbers() {
        let rows = make_rc_rows(&[&["-3.14"], &["2.5"], &["-0.001"]]);
        let result = extract_column_values(&rows, &[0, 1, 2], 0);
        assert_eq!(result, vec![(0, -3.14), (1, 2.5), (2, -0.001)]);
    }

    // --- extract_scatter_pairs ---

    #[test]
    fn scatter_pairs_basic() {
        let rows = make_rc_rows(&[&["1", "10"], &["2", "20"], &["3", "30"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2], 0, 1);
        assert_eq!(result, vec![(1.0, 10.0), (2.0, 20.0), (3.0, 30.0)]);
    }

    #[test]
    fn scatter_pairs_skips_non_numeric_in_either_column() {
        let rows = make_rc_rows(&[&["1", "10"], &["abc", "20"], &["3", "xyz"], &["4", "40"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2, 3], 0, 1);
        assert_eq!(result, vec![(1.0, 10.0), (4.0, 40.0)]);
    }

    #[test]
    fn scatter_pairs_empty_indices() {
        let rows = make_rc_rows(&[&["1", "10"]]);
        let result = extract_scatter_pairs(&rows, &[], 0, 1);
        assert!(result.is_empty());
    }

    #[test]
    fn scatter_pairs_filters_nan_infinity() {
        let rows = make_rc_rows(&[&["1", "NaN"], &["inf", "2"], &["3", "4"]]);
        let result = extract_scatter_pairs(&rows, &[0, 1, 2], 0, 1);
        assert_eq!(result, vec![(3.0, 4.0)]);
    }

    // --- compute_column_stats ---

    #[test]
    fn column_stats_basic() {
        let rows = make_rc_rows(&[&["10"], &["20"], &["30"]]);
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
        let rows = make_rc_rows(&[&["42"]]);
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
        let rows = make_rc_rows(&[&["10"], &["abc"], &["30"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2], 0).unwrap();
        assert_eq!(stats.count, 2);
        assert!((stats.sum - 40.0).abs() < f64::EPSILON);
    }

    #[test]
    fn column_stats_no_numeric_values() {
        let rows = make_rc_rows(&[&["abc"], &["def"]]);
        assert!(compute_column_stats(&rows, &[0, 1], 0).is_none());
    }

    #[test]
    fn column_stats_empty_indices() {
        let rows = make_rc_rows(&[&["10"]]);
        assert!(compute_column_stats(&rows, &[], 0).is_none());
    }

    #[test]
    fn column_stats_respects_filtered_indices() {
        let rows = make_rc_rows(&[&["10"], &["20"], &["30"], &["40"]]);
        let stats = compute_column_stats(&rows, &[1, 3], 0).unwrap();
        assert_eq!(stats.count, 2);
        assert!((stats.sum - 60.0).abs() < f64::EPSILON);
        assert!((stats.min - 20.0).abs() < f64::EPSILON);
        assert!((stats.max - 40.0).abs() < f64::EPSILON);
        assert!((stats.median - 30.0).abs() < f64::EPSILON); // (20+40)/2
    }

    #[test]
    fn column_stats_median_odd() {
        let rows = make_rc_rows(&[&["5"], &["1"], &["3"], &["4"], &["2"]]);
        let stats = compute_column_stats(&rows, &[0, 1, 2, 3, 4], 0).unwrap();
        assert!((stats.median - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn column_stats_median_even() {
        let rows = make_rc_rows(&[&["4"], &["1"], &["3"], &["2"]]);
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

    // --- filter_rows_regex ---

    #[test]
    fn filter_rows_regex_basic() {
        let rows = make_rc_rows(&[&["Tokyo"], &["Osaka"], &["Kyoto"]]);
        let indices = vec![0, 1, 2];
        let result = filter_rows_regex(&rows, &indices, "to", None).unwrap();
        assert_eq!(result, vec![0, 2]); // Tokyo, Kyoto
    }

    #[test]
    fn filter_rows_regex_column_specific() {
        let rows = make_rc_rows(&[&["Alice", "Tokyo"], &["Bob", "Osaka"], &["Carol", "Tokyo"]]);
        let indices = vec![0, 1, 2];
        let result = filter_rows_regex(&rows, &indices, "tokyo", Some(1)).unwrap();
        assert_eq!(result, vec![0, 2]);
    }

    #[test]
    fn filter_rows_regex_empty_pattern() {
        let rows = make_rc_rows(&[&["a"], &["b"]]);
        let indices = vec![0, 1];
        let result = filter_rows_regex(&rows, &indices, "", None).unwrap();
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn filter_rows_regex_invalid_pattern() {
        let rows = make_rc_rows(&[&["a"]]);
        assert!(filter_rows_regex(&rows, &[0], "[bad", None).is_err());
    }

    #[test]
    fn filter_rows_regex_respects_indices() {
        let rows = make_rc_rows(&[&["Tokyo"], &["Osaka"], &["Kyoto"]]);
        let indices = vec![0, 1]; // only Tokyo, Osaka
        let result = filter_rows_regex(&rows, &indices, "to", None).unwrap();
        assert_eq!(result, vec![0]); // only Tokyo
    }

    #[test]
    fn filter_rows_regex_out_of_range_col() {
        // target_col=Some(99) is beyond any row's columns — should match nothing
        let rows = make_rc_rows(&[&["Tokyo"], &["Osaka"]]);
        let indices = vec![0, 1];
        let result = filter_rows_regex(&rows, &indices, "Tokyo", Some(99)).unwrap();
        assert_eq!(result, Vec::<usize>::new());
    }

    // --- export_json ---

    #[test]
    fn export_json_basic() {
        let rows = make_rc_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        let result = export_json(&["name".into(), "age".into()], &rows, &[0, 1], &[0, 1]);
        assert_eq!(
            result,
            "[\n  {\"name\": \"Alice\", \"age\": \"30\"},\n  {\"name\": \"Bob\", \"age\": \"25\"}\n]\n"
        );
    }

    #[test]
    fn export_json_special_chars() {
        let rows = make_rc_rows(&[&["say \"hi\"", "line1\nline2"]]);
        let result = export_json(&["msg".into(), "note".into()], &rows, &[0], &[0, 1]);
        assert!(result.contains("say \\\"hi\\\""));
        assert!(result.contains("line1\\nline2"));
    }

    #[test]
    fn export_json_empty_rows() {
        let rows = make_rc_rows(&[&["a"]]);
        let result = export_json(&["col".into()], &rows, &[], &[0]);
        assert_eq!(result, "[]\n");
    }

    #[test]
    fn export_json_visible_cols() {
        let rows = make_rc_rows(&[&["Alice", "30", "Tokyo"]]);
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
        let rows = make_rc_rows(&[&["Alice", "30"], &["Bob", "25"]]);
        let result = export_markdown(&["name".into(), "age".into()], &rows, &[0, 1], &[0, 1]);
        assert_eq!(
            result,
            "| name | age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n"
        );
    }

    #[test]
    fn export_markdown_pipe_escape() {
        let rows = make_rc_rows(&[&["a|b"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("a\\|b"));
    }

    #[test]
    fn export_markdown_empty_rows() {
        let rows = make_rc_rows(&[&["a"]]);
        let result = export_markdown(&["col".into()], &rows, &[], &[0]);
        assert_eq!(result, "| col |\n| --- |\n");
    }

    #[test]
    fn export_markdown_visible_cols() {
        let rows = make_rc_rows(&[&["Alice", "30", "Tokyo"]]);
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
        let rows = make_rc_rows(&[&["a\x00b\x08c\x1f"]]);
        let result = export_json(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("a\\u0000b\\u0008c\\u001f"));
    }

    #[test]
    fn export_markdown_newline_in_cell() {
        let rows = make_rc_rows(&[&["line1\nline2"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("| line1 line2 |"));
    }

    #[test]
    fn export_json_out_of_bounds_index() {
        let rows = make_rc_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[0, 99], &[0]);
        // Row index 99 is skipped safely; only Alice appears
        assert_eq!(result, "[\n  {\"name\": \"Alice\"}\n]\n");
    }

    #[test]
    fn export_json_header_escape() {
        let rows = make_rc_rows(&[&["val"]]);
        let result = export_json(&["col\"name".into()], &rows, &[0], &[0]);
        assert!(result.contains("\"col\\\"name\""));
    }

    #[test]
    fn export_markdown_header_pipe_escape() {
        let rows = make_rc_rows(&[&["val"]]);
        let result = export_markdown(&["col|name".into()], &rows, &[0], &[0]);
        assert!(result.contains("col\\|name"));
    }

    #[test]
    fn export_json_empty_visible_cols() {
        let rows = make_rc_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[0], &[]);
        assert_eq!(result, "[]\n");
    }

    #[test]
    fn export_markdown_empty_visible_cols() {
        let rows = make_rc_rows(&[&["Alice"]]);
        let result = export_markdown(&["name".into()], &rows, &[0], &[]);
        assert_eq!(result, "");
    }

    #[test]
    fn export_json_out_of_bounds_first() {
        // When the first filtered index is out of bounds, the written counter
        // prevents a leading comma from appearing in the output.
        let rows = make_rc_rows(&[&["Alice"]]);
        let result = export_json(&["name".into()], &rows, &[99, 0], &[0]);
        assert_eq!(result, "[\n  {\"name\": \"Alice\"}\n]\n");
    }

    #[test]
    fn export_markdown_cr_in_cell() {
        let rows = make_rc_rows(&[&["line1\r\nline2"]]);
        let result = export_markdown(&["val".into()], &rows, &[0], &[0]);
        assert!(result.contains("| line1  line2 |"));
    }
}
