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

/// 读取区域（1 基、闭区间；`A1:C10` 或单格 `B2`；支持 `$` 前缀）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRange {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

/// 解析 `A1:C10`（或单格 `B2`）形式的区域。
pub fn parse_range(text: &str) -> Result<ReadRange, ToolError> {
    let (start, end) = match text.trim().split_once(':') {
        Some((start, end)) => (start, end),
        None => (text, text),
    };
    let (start_col, start_row) = parse_cell(start)?;
    let (end_col, end_row) = parse_cell(end)?;
    if start_row > end_row || start_col > end_col {
        return Err(invalid("range start must not be after range end"));
    }
    Ok(ReadRange {
        start_row,
        start_col,
        end_row,
        end_col,
    })
}

fn parse_cell(text: &str) -> Result<(usize, usize), ToolError> {
    let text = text.trim().replace('$', "");
    let split = text
        .find(|ch: char| ch.is_ascii_digit())
        .ok_or_else(|| invalid(format!("invalid cell reference '{text}'")))?;
    let (letters, digits) = text.split_at(split);
    if letters.is_empty()
        || digits.is_empty()
        || !letters.chars().all(|ch| ch.is_ascii_alphabetic())
        || !digits.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(invalid(format!("invalid cell reference '{text}'")));
    }
    let mut column = 0usize;
    for ch in letters.chars() {
        column = column * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    let row: usize = digits
        .parse()
        .map_err(|_| invalid(format!("invalid cell reference '{text}'")))?;
    if row == 0 {
        return Err(invalid("row numbers start at 1"));
    }
    Ok((column - 1, row - 1))
}

/// 读取工作簿：`[{ name, rows, truncated }]`，单元格保持原始类型（数字/字符串/布尔/空）。
/// `range` 为可选的 1 基闭区间；范围外的行列被跳过，行列上限按过滤后的数量计算。
pub fn read_workbook(path: &Path, range: Option<ReadRange>) -> Result<Value, ToolError> {
    let mut workbook =
        open_workbook_auto(path).map_err(|error| invalid(format!("open workbook: {error}")))?;
    let mut sheets = Vec::new();
    for name in workbook.sheet_names().to_owned() {
        let sheet_range = workbook
            .worksheet_range(&name)
            .map_err(|error| invalid(format!("read sheet {name}: {error}")))?;
        let mut rows = Vec::new();
        let mut truncated = false;
        for (row_index, row) in sheet_range.rows().enumerate() {
            if rows.len() >= MAX_ROWS {
                truncated = true;
                break;
            }
            if let Some(range) = range {
                if row_index < range.start_row {
                    continue;
                }
                if row_index > range.end_row {
                    break;
                }
            }
            let mut cells = Vec::new();
            for (column_index, cell) in row.iter().enumerate() {
                if let Some(range) = range {
                    if column_index < range.start_col {
                        continue;
                    }
                    if column_index > range.end_col {
                        break;
                    }
                }
                if cells.len() >= MAX_COLS {
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

/// 单列写出选项：宽度（字符数）与 Excel 数字格式代码（如 `#,##0.00`、`0.0%`）。
#[derive(Debug, Clone, Default)]
pub struct ColumnOptions {
    pub width: Option<f64>,
    pub number_format: Option<String>,
}

/// 写出规格：一个工作表的名字与行数据（单元格可为字符串/数字/布尔/空）。
#[derive(Debug, Clone, Default)]
pub struct SheetSpec {
    pub name: String,
    pub rows: Vec<Vec<Value>>,
    /// 首行作为表头加粗。
    pub header_row: bool,
    /// 冻结首行（可与 `header_row` 独立使用）。
    pub freeze_header: bool,
    /// 按列（从 0 开始）指定宽度与数字格式；未列出的列使用默认样式。
    pub columns: Vec<ColumnOptions>,
}

/// 数字格式表：格式代码 → numFmtId（自定义格式从 164 开始）。
#[derive(Default)]
struct NumberFormats(Vec<String>);

impl NumberFormats {
    fn intern(&mut self, code: &str) -> u32 {
        if let Some(index) = self.0.iter().position(|existing| existing == code) {
            return 164 + index as u32;
        }
        self.0.push(code.to_string());
        164 + (self.0.len() - 1) as u32
    }

    fn id(&self, code: &str) -> u32 {
        self.0
            .iter()
            .position(|existing| existing == code)
            .map(|index| 164 + index as u32)
            .unwrap_or(0)
    }
}

/// 样式表：(`bold`, `numFmtId`) → cellXfs 下标；下标 0 固定为默认样式。
#[derive(Default)]
struct StyleTable(Vec<(bool, u32)>);

impl StyleTable {
    fn intern(&mut self, bold: bool, format: u32) -> usize {
        if let Some(index) = self.0.iter().position(|entry| *entry == (bold, format)) {
            return index;
        }
        self.0.push((bold, format));
        self.0.len() - 1
    }

    fn id(&self, bold: bool, format: u32) -> usize {
        self.0
            .iter()
            .position(|entry| *entry == (bold, format))
            .unwrap_or(0)
    }
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
    // 先注册全部需要的样式，保证 cellXfs 覆盖每个单元格引用到的组合。
    let mut formats = NumberFormats::default();
    let mut styles = StyleTable::default();
    styles.intern(false, 0);
    for sheet in sheets {
        if sheet.header_row {
            styles.intern(true, 0);
        }
        for column in &sheet.columns {
            if let Some(code) = column.number_format.as_deref().map(str::trim) {
                if !code.is_empty() {
                    let format = formats.intern(code);
                    styles.intern(false, format);
                    if sheet.header_row {
                        styles.intern(true, format);
                    }
                }
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
    write_entry(
        &mut zip,
        "xl/styles.xml",
        styles_xml(&formats, &styles).as_bytes(),
    )?;
    for (index, sheet) in sheets.iter().enumerate() {
        write_entry(
            &mut zip,
            &format!("xl/worksheets/sheet{}.xml", index + 1),
            sheet_xml(sheet, &formats, &styles).as_bytes(),
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

fn styles_xml(formats: &NumberFormats, styles: &StyleTable) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
    );
    if !formats.0.is_empty() {
        xml.push_str(&format!(r#"<numFmts count="{}">"#, formats.0.len()));
        for (index, code) in formats.0.iter().enumerate() {
            xml.push_str(&format!(
                r#"<numFmt numFmtId="{}" formatCode="{}"/>"#,
                164 + index,
                escape(code)
            ));
        }
        xml.push_str("</numFmts>");
    }
    xml.push_str(
        r#"<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs>"#,
    );
    xml.push_str(&format!(r#"<cellXfs count="{}">"#, styles.0.len()));
    for (bold, format) in &styles.0 {
        let font = if *bold { 1 } else { 0 };
        if *format != 0 {
            xml.push_str(&format!(
                r#"<xf numFmtId="{format}" fontId="{font}" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>"#
            ));
        } else if *bold {
            xml.push_str(
                r#"<xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/>"#,
            );
        } else {
            xml.push_str(r#"<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>"#);
        }
    }
    xml.push_str(
        r#"</cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#,
    );
    xml
}

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
    xml.push_str(
        r#"<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
    );
    xml
}

fn sheet_xml(sheet: &SheetSpec, formats: &NumberFormats, styles: &StyleTable) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
    );
    if sheet.freeze_header {
        xml.push_str(
            r#"<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#,
        );
    }
    let widths: Vec<(usize, f64)> = sheet
        .columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| column.width.map(|width| (index, width.clamp(1.0, 255.0))))
        .collect();
    if !widths.is_empty() {
        xml.push_str("<cols>");
        for (index, width) in widths {
            xml.push_str(&format!(
                r#"<col min="{}" max="{}" width="{width}" customWidth="1"/>"#,
                index + 1,
                index + 1
            ));
        }
        xml.push_str("</cols>");
    }
    xml.push_str("<sheetData>");
    for (row_index, row) in sheet.rows.iter().enumerate() {
        let row_number = row_index + 1;
        let is_header = sheet.header_row && row_index == 0;
        xml.push_str(&format!(r#"<row r="{row_number}">"#));
        for (column_index, value) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(column_index), row_number);
            let format = if is_header {
                0
            } else {
                sheet
                    .columns
                    .get(column_index)
                    .and_then(|column| column.number_format.as_deref())
                    .map(|code| formats.id(code.trim()))
                    .unwrap_or(0)
            };
            let style = styles.id(is_header, format);
            if let Some(cell) = cell_xml(&reference, value, style) {
                xml.push_str(&cell);
            }
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml
}

fn cell_xml(reference: &str, value: &Value, style: usize) -> Option<String> {
    let style_attribute = if style > 0 {
        format!(r#" s="{style}""#)
    } else {
        String::new()
    };
    match value {
        Value::Null => None,
        Value::String(text) => Some(format!(
            r#"<c r="{reference}"{style_attribute} t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
            escape(text)
        )),
        Value::Number(number) => Some(format!(
            r#"<c r="{reference}"{style_attribute}><v>{number}</v></c>"#
        )),
        Value::Bool(value) => Some(format!(
            r#"<c r="{reference}"{style_attribute} t="b"><v>{}</v></c>"#,
            if *value { 1 } else { 0 }
        )),
        other => {
            let text = escape(&other.to_string());
            Some(format!(
                r#"<c r="{reference}"{style_attribute} t="inlineStr"><is><t xml:space="preserve">{text}</t></is></c>"#
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
            ..Default::default()
        }];
        let bytes = write_workbook(&path, &sheets).expect("write xlsx");
        assert!(bytes > 0);
        let read = read_workbook(&path, None).expect("read xlsx");
        let sheet = &read.as_array().expect("sheets")[0];
        assert_eq!(sheet["name"], "SAP");
        assert_eq!(sheet["rows"][0][0], "指标");
        assert_eq!(sheet["rows"][1][0], "CPU 已消耗");
        assert_eq!(sheet["rows"][2][1].as_f64(), Some(4188.0));
        assert_eq!(sheet["rows"][3][0], true);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn styles_widths_freeze_and_ranges_are_written_and_read() {
        use std::io::Read;

        let directory = std::env::temp_dir().join(format!("xlsx-style-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("styled.xlsx");
        let sheets = vec![SheetSpec {
            name: "报告".to_string(),
            rows: vec![
                vec![json!("指标"), json!("数值")],
                vec![json!("CPU"), json!(0.5)],
                vec![json!("MEM"), json!(0.75)],
            ],
            header_row: true,
            freeze_header: true,
            columns: vec![
                ColumnOptions {
                    width: Some(24.0),
                    number_format: None,
                },
                ColumnOptions {
                    width: Some(12.0),
                    number_format: Some("0.0%".to_string()),
                },
            ],
        }];
        write_workbook(&path, &sheets).expect("write styled xlsx");
        let file = File::open(&path).expect("open xlsx");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let mut styles = String::new();
        archive
            .by_name("xl/styles.xml")
            .expect("styles")
            .read_to_string(&mut styles)
            .expect("read styles");
        assert!(styles.contains("<b/>"));
        assert!(styles.contains("0.0%"));
        assert!(styles.contains(r#"applyNumberFormat="1""#));
        let mut sheet = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .expect("sheet")
            .read_to_string(&mut sheet)
            .expect("read sheet");
        assert!(sheet.contains(r#"<col min="1" max="1" width="24" customWidth="1"/>"#));
        assert!(sheet.contains(r#"state="frozen""#));

        let range = parse_range("B2:B3").expect("parse range");
        assert_eq!(
            range,
            ReadRange {
                start_row: 1,
                start_col: 1,
                end_row: 2,
                end_col: 1
            }
        );
        let read = read_workbook(&path, Some(range)).expect("read range");
        let rows = read[0]["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].as_f64(), Some(0.5));
        assert_eq!(rows[1][0].as_f64(), Some(0.75));
        assert!(rows[0].as_array().expect("row").len() == 1);
        assert!(parse_range("bad").is_err());
        assert!(parse_range("C1:A1").is_err());
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
