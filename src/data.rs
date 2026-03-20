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
}
