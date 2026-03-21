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
}

impl CsvData {
    pub(crate) fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, csv::Error> {
        let mut rdr = csv::ReaderBuilder::new().has_headers(true).from_reader(reader);
        let headers = rdr.headers()?.iter().map(|s| s.to_string()).collect();
        let rows = rdr
            .records()
            .map(|r| r.map(|record| record.iter().map(|s| s.to_string()).collect()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CsvData { headers, rows })
    }
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
        calamine::Data::DateTime(dt) => dt
            .as_datetime()
            .map(|d| d.to_string())
            .unwrap_or_else(|| {
                let v = dt.as_f64();
                eprintln!("Warning: could not parse DateTime value {}", v);
                format!("{}", v)
            }),
        calamine::Data::DateTimeIso(s) => s.clone(),
        calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("{}", e),
    }
}

impl CsvData {
    pub(crate) fn from_xlsx<P: AsRef<std::path::Path>>(path: P) -> Result<Self, String> {
        use calamine::{Reader, open_workbook_auto};
        let mut workbook = open_workbook_auto(&path)
            .map_err(|e| format!("failed to open workbook: {}", e))?;
        let first_sheet = workbook.sheet_names().first()
            .ok_or_else(|| "workbook has no sheets".to_string())?
            .clone();
        let range = workbook.worksheet_range(&first_sheet)
            .map_err(|e| format!("failed to read sheet '{}': {}", first_sheet, e))?;

        let mut row_iter = range.rows();
        let headers = row_iter.next()
            .map(|row| row.iter().map(cell_to_string).collect())
            .unwrap_or_else(|| {
                eprintln!("Warning: sheet '{}' is empty", first_sheet);
                vec![]
            });
        let rows = row_iter
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();

        Ok(CsvData { headers, rows })
    }
}

/// Detect encoding and decode bytes to UTF-8.
/// Returns `None` if already valid UTF-8 (no conversion needed).
/// Returns `Some(Ok(decoded_bytes))` on successful transcoding.
/// Returns `Some(Err(msg))` if the detected encoding cannot losslessly decode the input.
pub(crate) fn decode_to_utf8(bytes: &[u8]) -> Option<Result<Vec<u8>, String>> {
    if std::str::from_utf8(bytes).is_ok() {
        return None;
    }

    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);

    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Some(Err(format!(
            "failed to decode file: detected encoding {} produced invalid characters",
            encoding.name()
        )));
    }
    Some(Ok(decoded.into_owned().into_bytes()))
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
        let result = decode_to_utf8(&sjis_bytes).unwrap().expect("should transcode without errors");
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
        assert_eq!(cell_to_string(&calamine::Data::String("hello".into())), "hello");
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
        assert_eq!(cell_to_string(&calamine::Data::Float(999_999_999_999_999.0)), "999999999999999");
        // At/above 1e15: uses f64 Display format to avoid i64 precision loss.
        // Note: both branches produce identical output at 1e15 because the value
        // is exactly representable in both f64 and i64. The threshold guards against
        // values where f64-to-i64 cast would silently lose precision.
        assert_eq!(cell_to_string(&calamine::Data::Float(1e15)), "1000000000000000");
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
        // The Some(Err) path (had_errors == true) is difficult to trigger reliably
        // because chardetng often selects Windows-1252, which accepts all byte values.
        let bytes: Vec<u8> = vec![0xC0, 0xC1];
        let result = decode_to_utf8(&bytes);
        assert!(result.is_some());
    }
}
