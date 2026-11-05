#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartType {
    Bar,
    Line,
    Scatter,
    Histogram,
}

pub(crate) const CHART_TYPES: [ChartType; 4] = [
    ChartType::Bar,
    ChartType::Line,
    ChartType::Scatter,
    ChartType::Histogram,
];

impl ChartType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ChartType::Bar => "Bar",
            ChartType::Line => "Line",
            ChartType::Scatter => "Scatter",
            ChartType::Histogram => "Histogram",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Ascending,
    Descending,
}

pub(crate) struct CsvData {
    pub(crate) headers: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    /// Original header row + rows before the promoted header (metadata section in files with mixed formats)
    pub(crate) metadata: Vec<Vec<String>>,
}

impl CsvData {
    pub(crate) fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, csv::Error> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(reader);
        let mut headers: Vec<String> = rdr.headers()?.iter().map(|s| s.to_string()).collect();
        let mut rows = rdr
            .records()
            .map(|r| r.map(|record| record.iter().map(|s| s.to_string()).collect()))
            .collect::<Result<Vec<Vec<String>>, _>>()?;

        // Detect the dominant field count among all rows (including the parsed header).
        // If a wider field count is strictly more frequent than the header's, promote the
        // first row matching that count to the header, saving earlier rows as metadata.
        let header_len = headers.len();
        let dominant_len = find_dominant_field_count(header_len, &rows);
        let mut metadata = Vec::new();
        if let Some(pos) = rows
            .iter()
            .position(|r| dominant_len > header_len && r.len() == dominant_len)
        {
            // Move rows before the promoted header into metadata (avoids cloning)
            let tail = rows.split_off(pos);
            metadata.push(std::mem::take(&mut headers));
            metadata.append(&mut rows);
            let mut tail_iter = tail.into_iter();
            headers = tail_iter.next().unwrap_or_default();
            rows = tail_iter.collect();
        }

        // Pad headers for any remaining rows wider than the header
        let max_fields = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for i in headers.len()..max_fields {
            headers.push(format!("Column {}", i + 1));
        }

        Ok(CsvData {
            headers,
            rows,
            metadata,
        })
    }
}

/// Find the field count that strictly dominates (more frequent than) the header's field count.
/// Returns `header_len` if no other count is strictly more frequent, or if multiple
/// non-header widths tie for the highest frequency (ambiguous case).
fn find_dominant_field_count(header_len: usize, rows: &[Vec<String>]) -> usize {
    let mut counts = std::collections::HashMap::<usize, usize>::new();
    if header_len > 0 {
        *counts.entry(header_len).or_default() += 1; // count the header row itself
    }
    for row in rows {
        if !row.is_empty() {
            *counts.entry(row.len()).or_default() += 1;
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
pub(crate) enum ChartData {
    Points(Vec<f64>),
    Bins(Vec<usize>),
    Pairs(Vec<(f64, f64)>),
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
        calamine::Data::DateTime(dt) => {
            dt.as_datetime().map(|d| d.to_string()).unwrap_or_else(|| {
                let v = dt.as_f64();
                eprintln!("Warning: could not parse DateTime value {}", v);
                format!("{}", v)
            })
        }
        calamine::Data::DateTimeIso(s) => s.clone(),
        calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("{}", e),
    }
}

impl CsvData {
    pub(crate) fn from_xlsx<P: AsRef<std::path::Path>>(path: P) -> Result<Self, String> {
        use calamine::{Reader, open_workbook_auto};
        let mut workbook =
            open_workbook_auto(&path).map_err(|e| format!("failed to open workbook: {}", e))?;
        let first_sheet = workbook
            .sheet_names()
            .first()
            .ok_or_else(|| "workbook has no sheets".to_string())?
            .clone();
        let range = workbook
            .worksheet_range(&first_sheet)
            .map_err(|e| format!("failed to read sheet '{}': {}", first_sheet, e))?;

        let mut row_iter = range.rows();
        let headers = row_iter
            .next()
            .map(|row| row.iter().map(cell_to_string).collect())
            .unwrap_or_else(|| {
                eprintln!("Warning: sheet '{}' is empty", first_sheet);
                vec![]
            });
        let rows = row_iter
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();

        Ok(CsvData {
            headers,
            rows,
            metadata: Vec::new(),
        })
    }
}

/// Detect encoding and decode bytes to UTF-8.
/// Returns `None` if already valid UTF-8 (no conversion needed).
/// Returns `Some(Ok(decoded_bytes))` on successful transcoding. Input that looks like damaged
/// UTF-8 (see `is_damaged_utf8`) has its invalid bytes replaced with U+FFFD. A warning goes to
/// stderr for that replacement and whenever non-UTF-8 input is decoded with a guessed encoding.
/// Returns `Some(Err(msg))` if the detected encoding cannot losslessly decode the input.
pub(crate) fn decode_to_utf8(bytes: &[u8]) -> Option<Result<Vec<u8>, String>> {
    let is_valid_utf8 = std::str::from_utf8(bytes).is_ok();
    // ISO-2022-JP is 7-bit with escape sequences, so it also passes UTF-8 validation.
    let may_be_iso_2022_jp = bytes.is_ascii() && bytes.contains(&0x1B);
    if is_valid_utf8 && !may_be_iso_2022_jp {
        return None;
    }

    if !is_valid_utf8 && is_damaged_utf8(bytes) {
        eprintln!("Warning: input contains invalid UTF-8 bytes; replaced with U+FFFD");
        return Some(Ok(String::from_utf8_lossy(bytes).into_owned().into_bytes()));
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
    if !is_valid_utf8 {
        // The guess can be wrong, so make it visible rather than silently showing mojibake.
        eprintln!(
            "Warning: input is not valid UTF-8; decoded as {}",
            actual_encoding.name()
        );
    }
    Some(Ok(decoded.into_owned().into_bytes()))
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

    #[test]
    fn parse_promotes_header_from_dominant_width() {
        // Metadata header has 2 fields, but the majority of rows have 5.
        // The first 5-field row becomes the new header; earlier rows become metadata.
        let input = "key,value\nmeta1,v1\nmeta2,v2\nh1,h2,h3,h4,h5\n1,2,3,4,5\n6,7,8,9,10\n11,12,13,14,15\n";
        let data = CsvData::from_reader(input.as_bytes()).unwrap();
        assert_eq!(data.headers, vec!["h1", "h2", "h3", "h4", "h5"]);
        assert_eq!(data.rows.len(), 3);
        assert_eq!(data.rows[0], vec!["1", "2", "3", "4", "5"]);
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
        let decoded = std::str::from_utf8(&result).unwrap();
        assert!(decoded.contains("名前"));
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
        // ExcelDateTime with a value that cannot be converted to NaiveDateTime
        // Negative serial dates are not valid Excel dates
        let dt = calamine::ExcelDateTime::new(-1.0, calamine::ExcelDateTimeType::DateTime, false);
        let result = cell_to_string(&calamine::Data::DateTime(dt));
        // Should fall back to the raw float value
        assert!(result.contains("-1"));
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
        String::from_utf8(decoded).unwrap()
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
