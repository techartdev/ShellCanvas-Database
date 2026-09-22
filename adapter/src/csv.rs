use crate::model::{Cell, Column};

const MAX_CSV_BYTES: usize = 16 * 1024 * 1024;

fn field(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn safe_text(value: &str, safe: bool) -> String {
    if safe
        && matches!(
            value.chars().next(),
            Some('=' | '+' | '-' | '@' | '\t' | '\r')
        )
    {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}
fn display(cell: &Cell, safe: bool) -> String {
    if cell.kind == "null" {
        return String::new();
    }
    if safe
        && matches!(
            cell.kind,
            "text" | "json" | "date" | "time" | "datetime" | "uuid" | "unsupported"
        )
    {
        return safe_text(&cell.display, true);
    }
    cell.display.clone()
}

pub fn encode(
    columns: &[Column],
    rows: &[Vec<Cell>],
    spreadsheet_safe: bool,
) -> Result<Vec<u8>, &'static str> {
    if columns.is_empty() || columns.len() > 256 || rows.len() > 10_000 {
        return Err("CSV export dimensions are outside the supported limits");
    }
    if rows.iter().any(|row| row.len() != columns.len()) {
        return Err("CSV rows do not match the result columns");
    }
    let mut text = String::from("\u{feff}");
    text.push_str(
        &columns
            .iter()
            .map(|v| field(&safe_text(&v.name, spreadsheet_safe)))
            .collect::<Vec<_>>()
            .join(","),
    );
    text.push_str("\r\n");
    for row in rows {
        text.push_str(
            &row.iter()
                .map(|v| field(&display(v, spreadsheet_safe)))
                .collect::<Vec<_>>()
                .join(","),
        );
        text.push_str("\r\n");
        if text.len() > MAX_CSV_BYTES {
            return Err("CSV export exceeds 16 MiB");
        }
    }
    Ok(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_unicode_null_and_formulas() {
        let columns = vec![
            Column {
                name: "=name".into(),
                database_type: "text".into(),
            },
            Column {
                name: "value".into(),
                database_type: "text".into(),
            },
        ];
        let rows = vec![
            vec![
                Cell {
                    kind: "text",
                    display: "Жоро, \"hi\"".into(),
                },
                Cell {
                    kind: "text",
                    display: "=1+1".into(),
                },
            ],
            vec![
                Cell {
                    kind: "null",
                    display: String::new(),
                },
                Cell {
                    kind: "number",
                    display: "-12".into(),
                },
            ],
        ];
        let csv = String::from_utf8(encode(&columns, &rows, true).unwrap()).unwrap();
        assert_eq!(
            csv,
            "\u{feff}'=name,value\r\n\"Жоро, \"\"hi\"\"\",'=1+1\r\n,-12\r\n"
        );
        let raw = String::from_utf8(encode(&columns, &rows, false).unwrap()).unwrap();
        assert!(raw.starts_with("\u{feff}=name,value"));
        assert!(raw.contains(",=1+1\r\n"));
    }
}
