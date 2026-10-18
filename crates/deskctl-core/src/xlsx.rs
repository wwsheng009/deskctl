//! 极简 XLSX 读取/写出。
//!
//! - 读取：calamine（xlsx/xls/ods 等），输出 JSON 化的工作表与单元格值，带行列上限防爆。
//! - 写出：zip + 最小 OOXML（inline 字符串，无需共享字符串表），Excel/WPS/LibreOffice 可打开。

use std::fs::File;
use std::io::Write;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use deskctl_protocol::{ErrorCode, ToolError};
use serde_json::{json, Value};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

/// 读取上限：每表最多返回的行/列数（超出置 `truncated`）。
pub const MAX_ROWS: usize = 500;
pub const MAX_COLS: usize = 100;
const MAX_WRITE_ROWS: usize = 20_000;
const MAX_WRITE_COLS: usize = 500;

/// 读取工作簿：`[{ name, rows, truncated }]`，单元格保持原始类型（数字/字符串/布尔/空）。
pub fn read_workbook(path: &Path) -> Result<Value, ToolError> {
    let mut workbook =
        open_workbook_auto(path).map_err(|error| invalid(format!("open workbook: {error}")))?;
    let mut sheets = Vec::new();
    for name in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|error| invalid(format!("read sheet {name}: {error}")))?;
        let mut rows = Vec::new();
        let mut truncated = false;
        for (row_index, row) in range.rows().enumerate() {
            if row_index >= MAX_ROWS {
                truncated = true;
                break;
            }
            let mut cells = Vec::new();
            for (column_index, cell) in row.iter().enumerate() {
                if column_index >= MAX_COLS {
                    truncated = true;
                    break;
                }
                cells.push(cell_value(cell));
            }
            rows.push(Value::Array(cells));
        }
        sheets.push(json!({ "name": name, "rows": rows, "truncated": truncated }));
    }
    Ok(Value::Array(sheets))
}

fn cell_value(cell: &Data) -> Value {
    match cell {
        Data::Empty => Value::Null,
        Data::String(value) => json!(value),
        Data::Float(value) => json!(value),
        Data::Int(value) => json!(value),
        Data::Bool(value) => json!(value),
        Data::DateTime(value) => json!(value.as_f64()),
        Data::DateTimeIso(value) | Data::DurationIso(value) => json!(value),
        Data::Error(error) => json!(format!("{error:?}")),
    }
}

/// 写出规格：一个工作表的名字与行数据（单元格可为字符串/数字/布尔/空）。
pub struct SheetSpec {
    pub name: String,
    pub rows: Vec<Vec<Value>>,
}

/// 写出 `.xlsx`，返回字节数。
pub fn write_workbook(path: &Path, sheets: &[SheetSpec]) -> Result<u64, ToolError> {
    if sheets.is_empty() {
        return Err(invalid("at least one sheet is required"));
    }
    for sheet in sheets {
        if sheet.rows.len() > MAX_WRITE_ROWS {
            return Err(invalid(format!(
                "sheet '{}' exceeds {MAX_WRITE_ROWS} rows",
                sheet.name
            )));
        }
        for row in &sheet.rows {
            if row.len() > MAX_WRITE_COLS {
                return Err(invalid(format!(
                    "sheet '{}' exceeds {MAX_WRITE_COLS} columns",
                    sheet.name
                )));
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| internal(format!("create output directory: {error}")))?;
    }
    let file = File::create(path).map_err(|error| internal(format!("create workbook: {error}")))?;
    let mut zip = ZipWriter::new(file);
    write_entry(
        &mut zip,
        "[Content_Types].xml",
        content_types(sheets.len()).as_bytes(),
    )?;
    write_entry(&mut zip, "_rels/.rels", ROOT_RELS.as_bytes())?;
    write_entry(&mut zip, "xl/workbook.xml", workbook_xml(sheets).as_bytes())?;
    write_entry(
        &mut zip,
        "xl/_rels/workbook.xml.rels",
        workbook_rels(sheets.len()).as_bytes(),
    )?;
    write_entry(&mut zip, "xl/styles.xml", STYLES.as_bytes())?;
    for (index, sheet) in sheets.iter().enumerate() {
        write_entry(
            &mut zip,
            &format!("xl/worksheets/sheet{}.xml", index + 1),
            sheet_xml(sheet).as_bytes(),
        )?;
    }
    zip.finish()
        .map_err(|error| internal(format!("finalize workbook: {error}")))?;
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| internal(format!("read workbook metadata: {error}")))
}
fn content_types(sheet_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
    );
    for index in 1..=sheet_count {
        xml.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        ));
    }
    xml.push_str("</Types>");
    xml
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="1"><xf/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

fn workbook_xml(sheets: &[SheetSpec]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
    );
    for (index, sheet) in sheets.iter().enumerate() {
        let name = escape(&sheet.name);
        let name = if name.chars().count() > 31 {
            name.chars().take(31).collect::<String>()
        } else {
            name
        };
        xml.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{}" r:id="rId{}"/>"#,
            index + 1,
            index + 1
        ));
    }
    xml.push_str("</sheets></workbook>");
    xml
}

fn workbook_rels(sheet_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for index in 1..=sheet_count {
        xml.push_str(&format!(
            r#"<Relationship Id="rId{index}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{index}.xml"/>"#
        ));
    }
    xml.push_str(&format!(
        r#"<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#
    ));
    xml
}

fn sheet_xml(sheet: &SheetSpec) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (row_index, row) in sheet.rows.iter().enumerate() {
        let row_number = row_index + 1;
        xml.push_str(&format!(r#"<row r="{row_number}">"#));
        for (column_index, value) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(column_index), row_number);
            if let Some(cell) = cell_xml(&reference, value) {
                xml.push_str(&cell);
            }
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml
}

fn cell_xml(reference: &str, value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(format!(
            r#"<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
            escape(text)
        )),
        Value::Number(number) => Some(format!(r#"<c r="{reference}"><v>{number}</v></c>"#)),
        Value::Bool(value) => Some(format!(
            r#"<c r="{reference}" t="b"><v>{}</v></c>"#,
            if *value { 1 } else { 0 }
        )),
        other => {
            let text = escape(&other.to_string());
            Some(format!(
                r#"<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">{text}</t></is></c>"#
            ))
        }
    }
}

fn column_name(mut index: usize) -> String {
    let mut name = String::new();
    index += 1;
    while index > 0 {
        let remainder = (index - 1) % 26;
        name.insert(0, (b'A' + remainder as u8) as char);
        index = (index - 1) / 26;
    }
    name
}

fn write_entry(zip: &mut ZipWriter<File>, name: &str, data: &[u8]) -> Result<(), ToolError> {
    let options: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(name, options)
        .map_err(|error| internal(format!("write {name}: {error}")))?;
    zip.write_all(data)
        .map_err(|error| internal(format!("write {name}: {error}")))
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message)
}

fn internal(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::Internal, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_strings_numbers_and_bools() {
        let directory = std::env::temp_dir().join(format!("xlsx-rt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("rt.xlsx");
        let sheets = vec![SheetSpec {
            name: "SAP".to_string(),
            rows: vec![
                vec![json!("指标"), json!("值")],
                vec![json!("CPU 已消耗"), json!(0.16)],
                vec![json!("内存可用 MB"), json!(4188)],
                vec![json!(true), Value::Null],
            ],
        }];
        let bytes = write_workbook(&path, &sheets).expect("write xlsx");
        assert!(bytes > 0);
        let read = read_workbook(&path).expect("read xlsx");
        let sheet = &read.as_array().expect("sheets")[0];
        assert_eq!(sheet["name"], "SAP");
        assert_eq!(sheet["rows"][0][0], "指标");
        assert_eq!(sheet["rows"][1][0], "CPU 已消耗");
        assert_eq!(sheet["rows"][2][1].as_f64(), Some(4188.0));
        assert_eq!(sheet["rows"][3][0], true);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn column_names_span_multiple_letters() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(701), "ZZ");
    }
}
