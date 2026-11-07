#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Bar,
    Line,
    Scatter,
    Histogram,
}

pub const CHART_TYPES: [ChartType; 4] = [
    ChartType::Bar,
    ChartType::Line,
    ChartType::Scatter,
    ChartType::Histogram,
];

impl ChartType {
    pub fn label(self) -> &'static str {
        match self {
            ChartType::Bar => "Bar",
            ChartType::Line => "Line",
            ChartType::Scatter => "Scatter",
            ChartType::Histogram => "Histogram",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Cell text of all rows, packed so that a cell costs one offset instead of a `String`.
///
/// Rows are stored one after another rather than column by column, so a few rows with more fields
/// than the rest do not add a full column of offsets for every row.
#[derive(Debug)]
pub struct Table {
    /// All cells, row by row, without separators.
    text: String,
    /// Cell `i` is `text[cell_bounds[i]..cell_bounds[i + 1]]`.
    cell_bounds: Vec<usize>,
    /// Row `r` holds cells `row_starts[r]..row_starts[r + 1]`.
    row_starts: Vec<usize>,
    /// Number of cells in the widest row.
    column_count: usize,
}

/// One row of a `Table`. A row may have fewer cells than `Table::column_count`.
#[derive(Clone, Copy)]
pub struct Row<'a> {
    text: &'a str,
    /// The bounds of this row's cells, one more than the number of cells.
    bounds: &'a [usize],
}

impl<'a> Row<'a> {
    /// `None` past the end of the row, like `Vec::get`.
    pub fn get(&self, col: usize) -> Option<&'a str> {
        let start = *self.bounds.get(col)?;
        let end = *self.bounds.get(col + 1)?;
        Some(&self.text[start..end])
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'a str> + Clone + use<'a> {
        let text = self.text;
        self.bounds.windows(2).map(move |w| &text[w[0]..w[1]])
    }
}

impl Table {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            cell_bounds: vec![0],
            row_starts: vec![0],
            column_count: 0,
        }
    }

    pub fn row_count(&self) -> usize {
        self.row_starts.len() - 1
    }

    pub(crate) fn column_count(&self) -> usize {
        self.column_count
    }

    pub fn row(&self, row: usize) -> Option<Row<'_>> {
        let first = *self.row_starts.get(row)?;
        let last = *self.row_starts.get(row + 1)?;
        Some(Row {
            text: &self.text,
            bounds: &self.cell_bounds[first..=last],
        })
    }

    /// `None` when the row is out of range or has fewer than `col + 1` cells.
    pub(crate) fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.row(row)?.get(col)
    }

    pub(crate) fn row_lens(&self) -> impl Iterator<Item = usize> + '_ {
        self.row_starts.windows(2).map(|w| w[1] - w[0])
    }

    /// Append a parsed record without allocating a `String` per field.
    pub(crate) fn push_record(&mut self, record: &csv::StringRecord) {
        let base = self.text.len();
        self.text.push_str(record.as_slice());
        // `as_slice` joins the fields without separators, so field ends are offsets into it.
        self.cell_bounds.extend((0..record.len()).map(|i| {
            base + record
                .range(i)
                .expect("field index is below record.len()")
                .end
        }));
        self.finish_row();
    }

    pub(crate) fn push_row<I>(&mut self, cells: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        for cell in cells {
            self.text.push_str(cell.as_ref());
            self.cell_bounds.push(self.text.len());
        }
        self.finish_row();
    }

    fn finish_row(&mut self) {
        let cell_count = self.cell_bounds.len() - 1;
        let row_len = cell_count - self.row_starts.last().copied().unwrap_or(0);
        self.row_starts.push(cell_count);
        self.column_count = self.column_count.max(row_len);
        self.debug_check();
    }

    /// Remove the first `n` rows in place. Used once at load time when a header is promoted.
    pub(crate) fn drop_leading_rows(&mut self, n: usize) {
        let n = n.min(self.row_count());
        if n == 0 {
            return;
        }
        let first_cell = self.row_starts[n];
        let first_byte = self.cell_bounds[first_cell];
        self.text.drain(..first_byte);
        self.cell_bounds.drain(..first_cell);
        self.row_starts.drain(..n);
        for bound in &mut self.cell_bounds {
            *bound -= first_byte;
        }
        for start in &mut self.row_starts {
            *start -= first_cell;
        }
        // Padding the header counts only the remaining rows, so the dropped ones must not count.
        self.column_count = self.row_lens().max().unwrap_or(0);
        self.debug_check();
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
        self.cell_bounds.shrink_to_fit();
        self.row_starts.shrink_to_fit();
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.cell_bounds.first(), Some(&0));
        debug_assert_eq!(self.cell_bounds.last(), Some(&self.text.len()));
        debug_assert_eq!(self.row_starts.first(), Some(&0));
        debug_assert_eq!(self.row_starts.last(), Some(&(self.cell_bounds.len() - 1)));
    }

    #[cfg(test)]
    pub(crate) fn from_rows(rows: &[&[&str]]) -> Self {
        let mut table = Self::new();
        for row in rows {
            table.push_row(row.iter());
        }
        table
    }

    /// Bytes reserved on the heap, to keep the layout from growing unnoticed.
    #[cfg(test)]
    fn heap_bytes(&self) -> usize {
        self.text.capacity()
            + (self.cell_bounds.capacity() + self.row_starts.capacity())
                * std::mem::size_of::<usize>()
    }
}

pub struct CsvData {
    pub headers: Vec<String>,
    pub rows: Table,
    /// Original header row + rows before the promoted header (metadata section in files with mixed formats)
    pub metadata: Vec<Vec<String>>,
    /// Problems found while loading the input, shown in the status bar and on stderr
    pub warnings: Vec<String>,
}

impl CsvData {
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, csv::Error> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(reader);
        let mut headers: Vec<String> = rdr.headers()?.iter().map(|s| s.to_string()).collect();
        let mut rows = Table::new();
        let mut record = csv::StringRecord::new();
        while rdr.read_record(&mut record)? {
            rows.push_record(&record);
        }

        // Detect the dominant field count among all rows (including the parsed header).
        // If a wider field count is strictly more frequent than the header's, promote the
        // first row matching that count to the header, saving earlier rows as metadata.
        let header_len = headers.len();
        let dominant_len = find_dominant_field_count(header_len, rows.row_lens());
        let mut metadata = Vec::new();
        let promoted = (dominant_len > header_len)
            .then(|| rows.row_lens().position(|len| len == dominant_len))
            .flatten();
        if let Some(pos) = promoted {
            let owned_row = |r: usize| -> Vec<String> {
                rows.row(r)
                    .map(|row| row.iter().map(str::to_string).collect())
                    .unwrap_or_default()
            };
            metadata.push(std::mem::take(&mut headers));
            metadata.extend((0..pos).map(owned_row));
            headers = owned_row(pos);
            rows.drop_leading_rows(pos + 1);
        }
        rows.shrink_to_fit();

        // Pad headers for any remaining rows wider than the header
        for i in headers.len()..rows.column_count() {
            headers.push(format!("Column {}", i + 1));
        }

        Ok(CsvData {
            headers,
            rows,
            metadata,
            warnings: Vec::new(),
        })
    }
}

/// Find the field count that strictly dominates (more frequent than) the header's field count.
/// Returns `header_len` if no other count is strictly more frequent, or if multiple
/// non-header widths tie for the highest frequency (ambiguous case).
fn find_dominant_field_count(
    header_len: usize,
    row_lens: impl IntoIterator<Item = usize>,
) -> usize {
    let mut counts = std::collections::HashMap::<usize, usize>::new();
    if header_len > 0 {
        *counts.entry(header_len).or_default() += 1; // count the header row itself
    }
    for len in row_lens {
        if len > 0 {
            *counts.entry(len).or_default() += 1;
        }
    }
    // Promote only when a single non-header field count is strictly more frequent than
    // the header's. If multiple non-header widths share the highest count (tied), the
    // result is ambiguous so we fall back to the original header.
    let header_count = counts.get(&header_len).copied().unwrap_or(0);
    let candidates: Vec<(usize, usize)> = counts
        .into_iter()
        .filter(|&(len, count)| len != header_len && count > header_count)
        .collect();
    let max_count = candidates.iter().map(|&(_, c)| c).max().unwrap_or(0);
    let winners: Vec<usize> = candidates
        .into_iter()
        .filter(|&(_, c)| c == max_count)
        .map(|(len, _)| len)
        .collect();
    winners
        .first()
        .copied()
        .filter(|_| winners.len() == 1)
        .unwrap_or(header_len)
}

#[derive(Clone)]
pub enum ChartData {
    Points(Vec<f64>),
    Bins(Vec<usize>),
    Pairs(Vec<(f64, f64)>),
}

/// One axis of a chart: the value range the plot maps to and the labels to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartAxis {
    pub lo: f64,
    pub hi: f64,
    /// Position from `lo` (0.0) to `hi` (1.0), and the label to draw there.
    pub ticks: Vec<(f32, String)>,
}

/// `x` is `None` when the horizontal axis is not a value (bar and line charts).
#[derive(Clone, Debug, PartialEq)]
pub struct ChartAxes {
    pub y: ChartAxis,
    pub x: Option<ChartAxis>,
}

fn cell_to_string(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                format!("{}", f)
            }
        }
        calamine::Data::Int(i) => format!("{}", i),
        calamine::Data::Bool(b) => format!("{}", b),
        calamine::Data::DateTime(dt) => dt
            .as_datetime()
            .map(|d| d.to_string())
            .unwrap_or_else(|| format!("{}", dt.as_f64())),
        calamine::Data::DateTimeIso(s) => s.clone(),
        calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("{}", e),
    }
}

/// GPUI breaks lines at newlines even with `whitespace_nowrap`, so they would make the status bar
/// taller.
fn empty_sheet_warning(sheet_name: &str) -> String {
    format!("Sheet '{}' is empty", sheet_name.replace(['\r', '\n'], " "))
}

/// Reports a single count, because one warning per cell could flood the status bar.
fn unparsable_datetime_warning<'a>(
    cells: impl Iterator<Item = &'a calamine::Data>,
) -> Option<String> {
    let count = cells
        .filter(|cell| matches!(cell, calamine::Data::DateTime(dt) if dt.as_datetime().is_none()))
        .count();
    (count > 0)
        .then(|| format!("Date cells that could not be parsed are shown as raw numbers: {count}"))
}

/// Spreadsheet format for `CsvData::from_xlsx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpreadsheetFormat {
    Xlsx,
    Xls,
}

impl CsvData {
    /// Takes a reader instead of a path so the core does not touch the file system, which wasm32
    /// does not have. The caller names the format: detecting it from the content would need a
    /// cloneable reader (the whole file in memory) and would hide the parser's error behind
    /// "Cannot detect file format".
    pub fn from_xlsx<RS: std::io::Read + std::io::Seek>(
        reader: RS,
        format: SpreadsheetFormat,
    ) -> Result<Self, String> {
        use calamine::{Reader, Sheets, Xls, Xlsx, open_workbook_from_rs};
        let opened = match format {
            SpreadsheetFormat::Xlsx => open_workbook_from_rs::<Xlsx<_>, _>(reader)
                .map(Sheets::Xlsx)
                .map_err(|e| e.to_string()),
            SpreadsheetFormat::Xls => open_workbook_from_rs::<Xls<_>, _>(reader)
                .map(Sheets::Xls)
                .map_err(|e| e.to_string()),
        };
        let mut workbook = opened.map_err(|e| format!("failed to open workbook: {e}"))?;
        let first_sheet = workbook
            .sheet_names()
            .first()
            .ok_or_else(|| "workbook has no sheets".to_string())?
            .clone();
        let range = workbook
            .worksheet_range(&first_sheet)
            .map_err(|e| format!("failed to read sheet '{}': {}", first_sheet, e))?;

        let mut warnings = Vec::new();
        let mut row_iter = range.rows();
        let headers = row_iter
            .next()
            .map(|row| row.iter().map(cell_to_string).collect())
            .unwrap_or_else(|| {
                warnings.push(empty_sheet_warning(&first_sheet));
                vec![]
            });
        let mut rows = Table::new();
        for row in row_iter {
            rows.push_row(row.iter().map(cell_to_string));
        }
        rows.shrink_to_fit();
        warnings.extend(unparsable_datetime_warning(range.rows().flatten()));

        Ok(CsvData {
            headers,
            rows,
            metadata: Vec::new(),
            warnings,
        })
    }
}

#[derive(Debug)]
pub struct DecodedInput {
    pub bytes: Vec<u8>,
    pub warning: Option<String>,
    /// Name of the guessed encoding, set only when input that is not valid UTF-8 was decoded with it.
    /// Returned instead of printed, because the core cannot know where the caller shows messages.
    pub decoded_as: Option<&'static str>,
}

/// Detect encoding and decode bytes to UTF-8.
/// Returns `None` if no conversion is needed: valid UTF-8 that is not ISO-2022-JP.
/// Returns `Some(Ok(decoded))` on successful transcoding. Input that looks like damaged UTF-8
/// (see `is_damaged_utf8`) has its invalid bytes replaced with U+FFFD. `decoded.warning` is set
/// only for that replacement. A guessed encoding for non-UTF-8 input goes to `decoded.decoded_as`.
/// Returns `Some(Err(msg))` if the detected encoding cannot losslessly decode the input.
pub fn decode_to_utf8(bytes: &[u8]) -> Option<Result<DecodedInput, String>> {
    let is_valid_utf8 = std::str::from_utf8(bytes).is_ok();
    // ISO-2022-JP is 7-bit with escape sequences, so it also passes UTF-8 validation.
    let may_be_iso_2022_jp = bytes.is_ascii() && bytes.contains(&0x1B);
    if is_valid_utf8 && !may_be_iso_2022_jp {
        return None;
    }

    if !is_valid_utf8 && is_damaged_utf8(bytes) {
        return Some(Ok(DecodedInput {
            bytes: String::from_utf8_lossy(bytes).into_owned().into_bytes(),
            warning: Some("Input contains invalid UTF-8 bytes; replaced with U+FFFD".to_string()),
            decoded_as: None,
        }));
    }

    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let guessed = detector.guess(None, chardetng::Utf8Detection::Allow);
    // Escape sequences alone do not imply ISO-2022-JP (e.g. ANSI colors in a UTF-8 file).
    if is_valid_utf8 && guessed != encoding_rs::ISO_2022_JP {
        return None;
    }

    let (decoded, actual_encoding, had_errors) = guessed.decode(bytes);
    if had_errors {
        return Some(Err(format!(
            "failed to decode input: detected encoding {} produced invalid characters",
            actual_encoding.name()
        )));
    }
    Some(Ok(DecodedInput {
        bytes: decoded.into_owned().into_bytes(),
        warning: None,
        decoded_as: (!is_valid_utf8).then(|| actual_encoding.name()),
    }))
}

/// Valid legacy text can form a few UTF-8 sequences by accident (EUC-JP `高橋` is `B9 E2 B6 B6`:
/// one 3-byte character and one invalid sequence), so one wide character per invalid sequence
/// is not enough evidence of UTF-8.
const MIN_WIDE_CHARS_PER_INVALID_SEQUENCE: usize = 4;

/// Whether invalid input is more likely damaged UTF-8 than text in another encoding.
///
/// Only characters of 3 or more bytes count: half-width kana in Shift_JIS forms valid 2-byte
/// UTF-8 such as `C3 BD`, while Japanese text in UTF-8 reaches the ratio within a few lines.
/// Truncated input gets no exception, because Shift_JIS `齊木` (`EA 8E 96 D8`) without a trailing
/// newline is indistinguishable from truncated UTF-8.
fn is_damaged_utf8(bytes: &[u8]) -> bool {
    // With a UTF-8 BOM, `Encoding::decode` always selects UTF-8 and would reject the input.
    // The BOM survives replacement, and the CSV reader strips it later.
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return true;
    }

    let mut wide_chars = 0;
    let mut invalid_sequences = 0;
    for chunk in bytes.utf8_chunks() {
        wide_chars += chunk.valid().chars().filter(|c| c.len_utf8() >= 3).count();
        if !chunk.invalid().is_empty() {
            invalid_sequences += 1;
        }
    }
    wide_chars >= invalid_sequences * MIN_WIDE_CHARS_PER_INVALID_SEQUENCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(table: &Table, row: usize) -> Vec<&str> {
        table.row(row).expect("row in range").iter().collect()
    }

    fn all_cells(table: &Table) -> Vec<Vec<&str>> {
        (0..table.row_count()).map(|r| cells(table, r)).collect()
    }

    // --- Table ---

    #[test]
    fn table_empty() {
        let table = Table::new();
        assert_eq!(table.row_count(), 0);
        assert_eq!(table.column_count(), 0);
        assert!(table.row(0).is_none());
        assert_eq!(table.cell(0, 0), None);
    }

    #[test]
    fn table_ragged_rows() {
        let table = Table::from_rows(&[&["a", ""], &["b", "c", "d"], &[]]);
        assert_eq!(table.row_count(), 3);
        assert_eq!(table.column_count(), 3);
        assert_eq!(table.cell(0, 1), Some(""));
        assert_eq!(table.cell(0, 2), None);
        assert_eq!(table.cell(1, 2), Some("d"));
        // A row without fields is still a row, unlike one past the end.
        assert_eq!(table.row(2).map(|row| row.iter().count()), Some(0));
        assert_eq!(table.cell(2, 0), None);
        assert!(table.row(3).is_none());
        assert_eq!(table.cell(3, 0), None);
        assert_eq!(table.row_lens().collect::<Vec<_>>(), vec![2, 3, 0]);
    }

    #[test]
    fn table_multibyte_cells() {
        let table = Table::from_rows(&[&["名前", "é"], &["", "🍣"]]);
        assert_eq!(all_cells(&table), vec![vec!["名前", "é"], vec!["", "🍣"]]);
    }

    #[test]
    fn table_push_record_matches_push_row() {
        let input = "a,b\n\"x, y\",\"line1\nline2\"\n,名前,z\n";
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(input.as_bytes());
        let mut from_records = Table::new();
        let mut from_strings = Table::new();
        for record in rdr.records() {
            let record = record.unwrap();
            from_records.push_record(&record);
            from_strings.push_row(record.iter());
        }
        assert_eq!(
            all_cells(&from_records),
            vec![
                vec!["a", "b"],
                vec!["x, y", "line1\nline2"],
                vec!["", "名前", "z"]
            ]
        );
        assert_eq!(all_cells(&from_records), all_cells(&from_strings));
        assert_eq!(from_records.column_count(), 3);
    }

    #[test]
    fn table_drop_some_leading_rows_keeps_remaining_cells() {
        let rows: &[&[&str]] = &[
            &["名前", "", "a", "b", "c"],
            &[],
            &["é", ""],
            &["x", "🍣"],
            &[],
            &["", "y", "z"],
        ];
        let mut table = Table::from_rows(rows);
        table.drop_leading_rows(2);
        let expected: Vec<Vec<&str>> = rows[2..].iter().map(|row| row.to_vec()).collect();
        assert_eq!(all_cells(&table), expected);
        assert_eq!(table.column_count(), 3);
    }

    #[test]
    fn table_drop_no_rows() {
        let mut table = Table::from_rows(&[&["a"], &["b", "c"]]);
        table.drop_leading_rows(0);
        assert_eq!(all_cells(&table), vec![vec!["a"], vec!["b", "c"]]);
    }

    #[test]
    fn table_drop_all_rows_allows_pushing_again() {
        for n in [2, 5] {
            let mut table = Table::from_rows(&[&["a", "b"], &["c"]]);
            table.drop_leading_rows(n);
            assert_eq!(table.row_count(), 0);
            assert_eq!(table.column_count(), 0);
            assert_eq!(table.cell_bounds, vec![0]);
            assert_eq!(table.row_starts, vec![0]);
            table.push_row(["d"]);
            assert_eq!(all_cells(&table), vec![vec!["d"]]);
        }
    }

    #[test]
    fn table_heap_bytes_stays_compact() {
        const ROWS: usize = 1_000;
        const COLS: usize = 8;
        let mut table = Table::new();
        for r in 0..ROWS {
            table.push_row((0..COLS).map(|c| format!("{}", r * COLS + c)));
        }
        table.shrink_to_fit();
        let text_len = table.text.len();
        let limit = text_len + 9 * ROWS * COLS + 9 * ROWS;
        assert!(
            table.heap_bytes() <= limit,
            "{} > {limit}",
            table.heap_bytes()
        );
    }

    // --- find_dominant_field_count ---

    #[test]
    fn dominant_field_count_ignores_empty_rows() {
        assert_eq!(find_dominant_field_count(2, [3, 3, 0, 0, 0]), 3);
        assert_eq!(find_dominant_field_count(2, [2, 3]), 2);
    }

    // --- from_reader ---

    #[test]
    fn parse_basic_csv() {
        let input = "name,age,city\nAlice,30,Tokyo\nBob,25,Osaka\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["name", "age", "city"]);
        assert_eq!(data.rows.row_count(), 2);
        assert_eq!(cells(&data.rows, 0), vec!["Alice", "30", "Tokyo"]);
        assert_eq!(cells(&data.rows, 1), vec!["Bob", "25", "Osaka"]);
        assert_eq!(data.metadata, Vec::<Vec<String>>::new());
    }

    #[test]
    fn parse_single_column() {
        let input = "value\n1\n2\n3\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["value"]);
        assert_eq!(data.rows.row_count(), 3);
    }

    #[test]
    fn parse_empty_body() {
        let input = "a,b,c\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["a", "b", "c"]);
        assert_eq!(data.rows.row_count(), 0);
    }

    #[test]
    fn parse_quoted_fields() {
        let input = "name,note\nAlice,\"hello, world\"\nBob,\"line1\nline2\"\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.rows.cell(0, 1), Some("hello, world"));
        assert_eq!(data.rows.cell(1, 1), Some("line1\nline2"));
    }

    #[test]
    fn parse_empty_input() {
        let input = "";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert!(data.headers.is_empty());
        assert_eq!(data.rows.row_count(), 0);
    }

    #[test]
    fn parse_promotes_first_row_as_header() {
        // The first data row already has the dominant width, so it is promoted and only the
        // parsed header goes to metadata.
        let input = "k\nh1,h2,h3\n1,2,3\n4,5,6\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["h1", "h2", "h3"]);
        assert_eq!(data.metadata, vec![vec!["k".to_string()]]);
        assert_eq!(
            all_cells(&data.rows),
            vec![vec!["1", "2", "3"], vec!["4", "5", "6"]]
        );
    }

    #[test]
    fn parse_wide_metadata_row_does_not_pad_header() {
        let input = "k,v\nx,1,2,3,4,5,6\nh1,h2,h3\n1,2,3\n4,5,6\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["h1", "h2", "h3"]);
        assert_eq!(data.rows.column_count(), 3);
        assert_eq!(data.metadata.len(), 2);
        assert_eq!(data.metadata[1], vec!["x", "1", "2", "3", "4", "5", "6"]);
    }

    #[test]
    fn parse_pads_promoted_header_for_wider_rows() {
        let input = "k,v\nh1,h2,h3\n1,2,3\n4,5,6\n7,8,9,10\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["h1", "h2", "h3", "Column 4"]);
        assert_eq!(cells(&data.rows, 2), vec!["7", "8", "9", "10"]);
    }

    #[test]
    fn parse_promotes_header_from_dominant_width() {
        // Metadata header has 2 fields, but the majority of rows have 5.
        // The first 5-field row becomes the new header; earlier rows become metadata.
        let input = "key,value\nmeta1,v1\nmeta2,v2\nh1,h2,h3,h4,h5\n1,2,3,4,5\n6,7,8,9,10\n11,12,13,14,15\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["h1", "h2", "h3", "h4", "h5"]);
        assert_eq!(data.rows.row_count(), 3);
        assert_eq!(cells(&data.rows, 0), vec!["1", "2", "3", "4", "5"]);
        assert_eq!(data.metadata.len(), 3); // original header + 2 meta rows
        assert_eq!(data.metadata[0], vec!["key", "value"]);
        assert_eq!(data.metadata[1], vec!["meta1", "v1"]);
    }

    #[test]
    fn parse_pads_headers_when_no_dominant() {
        // Only one wider row — not dominant, so headers get padded instead.
        let input = "a,b\n1,2\n3,4\nx,y,z\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["a", "b", "Column 3"]);
    }

    #[test]
    fn parse_tied_counts_keep_original_header() {
        // 2-field: header + 2 rows = 3, 3-field: 3 rows = 3 → tied, keep original header
        let input = "a,b\n1,2\n3,4\nx,y,z\np,q,r\ns,t,u\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["a", "b", "Column 3"]);
        assert!(data.metadata.is_empty());
    }

    // --- decode_to_utf8 ---

    #[test]
    fn decode_utf8_passthrough() {
        let input = "name,age\nAlice,30\n";
        assert!(decode_to_utf8(input.as_bytes()).is_none());
    }

    #[test]
    fn decode_shift_jis() {
        // "名前" in Shift-JIS: 0x96BC 0x914F
        let sjis_bytes: Vec<u8> = vec![
            0x96, 0xBC, 0x91, 0x4F, // 名前
            0x0A, // newline
        ];
        let result = decode_to_utf8(&sjis_bytes)
            .unwrap()
            .expect("should transcode without errors");
        let decoded = std::str::from_utf8(&result.bytes).unwrap();
        assert!(decoded.contains("名前"));
        assert!(result.warning.is_none());
        assert_eq!(result.decoded_as, Some("Shift_JIS"));
    }

    #[test]
    fn decode_damaged_utf8_warns_about_replacement() {
        let bytes = [b"\xEF\xBB\xBFa,b\n1,".as_slice(), b"\xFF\n"].concat();
        let decoded = decode_to_utf8(&bytes)
            .expect("should need conversion")
            .expect("should decode without errors");
        let warning = decoded.warning.expect("replacement should warn");
        assert!(warning.contains("U+FFFD"), "{warning}");
        // Treated as UTF-8, so no other encoding was guessed.
        assert_eq!(decoded.decoded_as, None);
    }

    #[test]
    fn decode_iso_2022_jp_has_no_warning() {
        let (encoded, _, _) = encoding_rs::ISO_2022_JP.encode("名前,年齢\n田中,30\n");
        let decoded = decode_to_utf8(&encoded)
            .expect("should need conversion")
            .expect("should decode without errors");
        assert!(decoded.warning.is_none());
        // ISO-2022-JP is also valid UTF-8, so it is not reported as a guess.
        assert_eq!(decoded.decoded_as, None);
    }

    // --- from_xlsx ---

    #[test]
    fn from_xlsx_reports_unreadable_input() {
        for format in [SpreadsheetFormat::Xlsx, SpreadsheetFormat::Xls] {
            let Err(error) = CsvData::from_xlsx(std::io::Cursor::new(b"a,b\n1,2\n"), format) else {
                panic!("CSV text is not a workbook");
            };
            assert!(error.starts_with("failed to open workbook: "), "{error}");
        }
    }

    // --- warnings ---

    #[test]
    fn empty_sheet_warning_replaces_newlines() {
        assert_eq!(empty_sheet_warning("A\nB"), "Sheet 'A B' is empty");
    }

    fn unparsable_datetime() -> calamine::Data {
        calamine::Data::DateTime(calamine::ExcelDateTime::new(
            1e10,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ))
    }

    #[test]
    fn unparsable_datetime_warning_ignores_valid_cells() {
        let valid = calamine::Data::DateTime(calamine::ExcelDateTime::new(
            45000.0,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        let cells = [valid, calamine::Data::Float(1e10)];
        assert_eq!(unparsable_datetime_warning(cells.iter()), None);
    }

    #[test]
    fn unparsable_datetime_warning_counts_cells() {
        let cells = [
            unparsable_datetime(),
            calamine::Data::Float(1.0),
            unparsable_datetime(),
        ];
        let warning = unparsable_datetime_warning(cells.iter()).expect("should warn");
        assert!(warning.ends_with(": 2"), "{warning}");
    }

    #[test]
    fn decode_empty_input() {
        assert!(decode_to_utf8(&[]).is_none());
    }

    // --- cell_to_string ---

    #[test]
    fn cell_to_string_empty() {
        assert_eq!(cell_to_string(&calamine::Data::Empty), "");
    }

    #[test]
    fn cell_to_string_string() {
        assert_eq!(
            cell_to_string(&calamine::Data::String("hello".into())),
            "hello"
        );
    }

    #[test]
    fn cell_to_string_float_integer() {
        assert_eq!(cell_to_string(&calamine::Data::Float(42.0)), "42");
    }

    #[test]
    fn cell_to_string_float_decimal() {
        assert_eq!(cell_to_string(&calamine::Data::Float(3.14)), "3.14");
    }

    #[test]
    fn cell_to_string_int() {
        assert_eq!(cell_to_string(&calamine::Data::Int(100)), "100");
    }

    #[test]
    fn cell_to_string_bool() {
        assert_eq!(cell_to_string(&calamine::Data::Bool(true)), "true");
        assert_eq!(cell_to_string(&calamine::Data::Bool(false)), "false");
    }

    #[test]
    fn cell_to_string_float_large_integer() {
        // Below 1e15: uses `as i64` format (no decimal point)
        assert_eq!(
            cell_to_string(&calamine::Data::Float(999_999_999_999_999.0)),
            "999999999999999"
        );
        // At/above 1e15: uses f64 Display format to avoid i64 precision loss.
        // Note: both branches produce identical output at 1e15 because the value
        // is exactly representable in both f64 and i64. The threshold guards against
        // values where f64-to-i64 cast would silently lose precision.
        assert_eq!(
            cell_to_string(&calamine::Data::Float(1e15)),
            "1000000000000000"
        );
    }

    #[test]
    fn cell_to_string_datetime_fallback() {
        // A serial date this large is outside the range of NaiveDateTime
        let dt = calamine::ExcelDateTime::new(1e10, calamine::ExcelDateTimeType::DateTime, false);
        assert!(dt.as_datetime().is_none());
        let result = cell_to_string(&calamine::Data::DateTime(dt));
        // Should fall back to the raw float value
        assert_eq!(result, "10000000000");
    }

    #[test]
    fn decode_non_utf8_attempts_transcoding() {
        // Non-UTF-8 bytes trigger the transcoding path (returns Some).
        // Whether the result is Ok or Err depends on chardetng's encoding guess,
        // which is non-deterministic for short/ambiguous inputs. This test verifies
        // the function does not panic and returns Some (not None).
        let bytes: Vec<u8> = vec![0xC0, 0xC1];
        let result = decode_to_utf8(&bytes);
        assert!(result.is_some());
    }

    fn decoded_text(bytes: &[u8]) -> String {
        let decoded = decode_to_utf8(bytes)
            .expect("should need conversion")
            .expect("should decode without errors");
        String::from_utf8(decoded.bytes).unwrap()
    }

    fn encoded_row(encoding: &'static encoding_rs::Encoding, value: &str) -> Vec<u8> {
        let (encoded, _, had_unmappable) = encoding.encode(value);
        assert!(!had_unmappable, "{value} should map to {}", encoding.name());
        [b"id,name\n1,", encoded.as_ref(), b"\n"].concat()
    }

    #[test]
    fn decode_iso_2022_jp() {
        let input = "名前,年齢\n田中,30\n";
        let (encoded, _, _) = encoding_rs::ISO_2022_JP.encode(input);
        assert_eq!(decoded_text(&encoded), input);
    }

    #[test]
    fn decode_ansi_escape_passthrough() {
        assert!(decode_to_utf8(b"a,b\n\x1b[31mred\x1b[0m,1\n").is_none());
    }

    #[test]
    fn decode_truncated_utf8_replaces_invalid_bytes() {
        let input = "名前,年齢\n田中,30\n佐藤".as_bytes();
        let text = decoded_text(&input[..input.len() - 1]);
        assert!(text.contains("名前"));
        assert!(text.contains('\u{FFFD}'));
    }

    #[test]
    fn decode_invalid_byte_in_middle_replaces_invalid_bytes() {
        let bytes = ["名前".as_bytes(), &[0xFF], ",年齢\n".as_bytes()].concat();
        let text = decoded_text(&bytes);
        assert!(text.contains("名前"));
        assert!(text.contains("年齢"));
        assert!(text.contains('\u{FFFD}'));
    }

    #[test]
    fn short_damaged_utf8_is_left_to_detection() {
        let input = "id,name\n1,名前".as_bytes();
        assert!(!is_damaged_utf8(&input[..input.len() - 1]));
    }

    #[test]
    fn decode_bom_with_invalid_utf8_replaces_invalid_bytes() {
        let text = decoded_text(b"\xEF\xBB\xBFid,name\n1,A\xFF\n");
        assert!(text.contains('\u{FFFD}'));
        let data = CsvData::from_reader(text.as_bytes()).unwrap();
        assert_eq!(data.headers[0], "id");
    }

    #[test]
    fn decode_shift_jis_is_not_replaced() {
        let input = "名前,年齢\n田中,30\n";
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(input);
        assert_eq!(decoded_text(&encoded), input);
    }

    #[test]
    fn half_width_kana_shift_jis_is_not_damaged_utf8() {
        let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode("id,name\n1,ﾃｽ\n2,ﾃｽﾄ\n");
        assert!(!is_damaged_utf8(&encoded));
    }

    #[test]
    fn accidental_utf8_sequences_are_not_damaged_utf8() {
        let samples = [
            (encoding_rs::SHIFT_JIS, "齊藤"),
            (encoding_rs::EUC_JP, "高橋"),
            (encoding_rs::GBK, "瑜君"),
            (encoding_rs::BIG5, "梁雅"),
        ];
        for (encoding, value) in samples {
            let bytes = encoded_row(encoding, value);
            assert!(!is_damaged_utf8(&bytes), "{} {value}", encoding.name());
        }
    }

    #[test]
    fn shift_jis_resembling_truncated_utf8_is_not_damaged_utf8() {
        let with_newline = encoded_row(encoding_rs::SHIFT_JIS, "齊木");
        let without_newline = &with_newline[..with_newline.len() - 1];
        assert!(!is_damaged_utf8(&with_newline));
        assert!(!is_damaged_utf8(without_newline));
    }

    #[test]
    fn decode_error_names_encoding_selected_by_bom() {
        let error = decode_to_utf8(&[0xFF, 0xFE, 0x41])
            .expect("should need conversion")
            .expect_err("odd UTF-16 input should fail");
        assert!(error.contains("UTF-16LE"), "{error}");
    }
}
