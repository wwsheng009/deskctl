//! 极简 DOCX 生成器（OOXML + zip），不依赖 Office 或重量级文档库。
//!
//! 仅实现报告所需的最小集合：文档标题、小节标题、正文段落、内嵌 PNG 图片与图注。
//! 生成物是标准 `.docx`（OOXML 包），可被 Word / WPS / LibreOffice 直接打开。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use deskctl_protocol::{ErrorCode, ToolError};
use quick_xml::events::Event;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

const EMU_PER_PX: f64 = 9525.0; // 96 dpi
const EMU_PER_CM: f64 = 360_000.0;
const DEFAULT_MAX_WIDTH_CM: f64 = 15.0;

/// 文档内容块。
pub enum Block {
    /// 一级文档标题。
    Title(String),
    /// 小节标题。
    Heading(String),
    /// 正文段落。
    Paragraph(String),
    /// 表格（行 × 单元格文本）。
    Table(Vec<Vec<String>>),
    /// 内嵌 PNG 图片（含可选图注与宽度上限）。
    Image {
        png: Vec<u8>,
        width_px: u32,
        height_px: u32,
        caption: Option<String>,
        width_cm: Option<f64>,
    },
}

/// 文档级排版选项（页眉/页脚/页码）。
#[derive(Debug, Clone, Default)]
pub struct DocumentOptions {
    /// 页眉文本（居中）。
    pub header: Option<String>,
    /// 页脚文本（居中，与页码字段并列）。
    pub footer: Option<String>,
    /// 在页脚加入 `PAGE` 域（打开/打印时由 Word 更新为实际页码）。
    pub page_numbers: bool,
}

const IMAGE_RELATION: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const HEADER_RELATION: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const FOOTER_RELATION: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";

/// 写出 `.docx`，返回文件字节数。
pub fn write_document(
    path: &Path,
    blocks: &[Block],
    options: &DocumentOptions,
) -> Result<u64, ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| internal(format!("create output directory: {error}")))?;
    }
    let header = options
        .header
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let footer = options
        .footer
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let has_footer = footer.is_some() || options.page_numbers;
    let file = File::create(path).map_err(|error| internal(format!("create document: {error}")))?;
    let mut zip = ZipWriter::new(file);

    let mut document = String::new();
    document.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    document.push_str(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>"#,
    );

    let mut rels: Vec<(String, String, &'static str)> = Vec::new();
    let mut media: Vec<Vec<u8>> = Vec::new();
    let mut image_index = 0usize;
    let mut drawing_id = 1u32;
    for block in blocks {
        match block {
            Block::Title(text) => document.push_str(&paragraph(text, 32, true, false)),
            Block::Heading(text) => document.push_str(&paragraph(text, 26, true, false)),
            Block::Paragraph(text) => document.push_str(&paragraph(text, 0, false, false)),
            Block::Table(rows) => document.push_str(&table_xml(rows)),
            Block::Image {
                png,
                width_px,
                height_px,
                caption,
                width_cm,
            } => {
                image_index += 1;
                let (cx, cy) = extent(*width_px, *height_px, *width_cm);
                let relationship = format!("rIdImg{image_index}");
                document.push_str(&image_paragraph(
                    &relationship,
                    drawing_id,
                    cx,
                    cy,
                    caption.as_deref(),
                    image_index,
                ));
                drawing_id += 1;
                rels.push((
                    relationship,
                    format!("media/image{image_index}.png"),
                    IMAGE_RELATION,
                ));
                media.push(png.clone());
                if let Some(caption) = caption {
                    document.push_str(&paragraph(caption, 18, false, true));
                }
            }
        }
    }
    if header.is_some() || has_footer {
        document.push_str("<w:sectPr>");
        if header.is_some() {
            document.push_str(r#"<w:headerReference w:type="default" r:id="rIdHeader1"/>"#);
        }
        if has_footer {
            document.push_str(r#"<w:footerReference w:type="default" r:id="rIdFooter1"/>"#);
        }
        document.push_str(
            r#"<w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>"#,
        );
    }
    document.push_str("</w:body></w:document>");
    if header.is_some() {
        rels.push((
            "rIdHeader1".to_string(),
            "header1.xml".to_string(),
            HEADER_RELATION,
        ));
    }
    if has_footer {
        rels.push((
            "rIdFooter1".to_string(),
            "footer1.xml".to_string(),
            FOOTER_RELATION,
        ));
    }
    write_entry(
        &mut zip,
        "[Content_Types].xml",
        content_types(header.is_some(), has_footer).as_bytes(),
    )?;
    write_entry(&mut zip, "_rels/.rels", ROOT_RELS.as_bytes())?;
    write_entry(&mut zip, "word/document.xml", document.as_bytes())?;
    write_entry(
        &mut zip,
        "word/_rels/document.xml.rels",
        document_rels(&rels).as_bytes(),
    )?;
    if header.is_some() {
        write_entry(
            &mut zip,
            "word/header1.xml",
            header_footer_part("w:hdr", header, false).as_bytes(),
        )?;
    }
    if has_footer {
        write_entry(
            &mut zip,
            "word/footer1.xml",
            header_footer_part("w:ftr", footer, options.page_numbers).as_bytes(),
        )?;
    }
    for (index, png) in media.iter().enumerate() {
        write_entry(&mut zip, &format!("word/media/image{}.png", index + 1), png)?;
    }
    zip.finish()
        .map_err(|error| internal(format!("finalize document: {error}")))?;
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| internal(format!("read document metadata: {error}")))
}

fn content_types(header: bool, footer: bool) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    );
    if header {
        xml.push_str(r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>"#);
    }
    if footer {
        xml.push_str(r#"<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>"#);
    }
    xml.push_str("</Types>");
    xml
}

fn header_footer_part(root: &str, text: Option<&str>, page_numbers: bool) -> String {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="center"/></w:pPr>"#
    );
    if let Some(text) = text {
        xml.push_str(&format!(
            r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
            escape(text)
        ));
        if page_numbers {
            xml.push_str(r#"<w:r><w:t xml:space="preserve"> </w:t></w:r>"#);
        }
    }
    if page_numbers {
        xml.push_str(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        );
    }
    xml.push_str(&format!("</w:p></{root}>"));
    xml
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn document_rels(rels: &[(String, String, &'static str)]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, target, kind) in rels {
        xml.push_str(&format!(
            r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#
        ));
    }
    xml.push_str("</Relationships>");
    xml
}

fn write_entry(zip: &mut ZipWriter<File>, name: &str, data: &[u8]) -> Result<(), ToolError> {
    let options: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(name, options)
        .map_err(|error| internal(format!("write {name}: {error}")))?;
    zip.write_all(data)
        .map_err(|error| internal(format!("write {name}: {error}")))
}

fn paragraph(text: &str, size_half_pt: u32, bold: bool, italic: bool) -> String {
    let mut run_properties = String::from("<w:rPr>");
    if bold {
        run_properties.push_str("<w:b/>");
    }
    if italic {
        run_properties.push_str("<w:i/>");
    }
    if size_half_pt > 0 {
        run_properties.push_str(&format!(
            "<w:sz w:val=\"{size_half_pt}\"/><w:szCs w:val=\"{size_half_pt}\"/>"
        ));
    }
    run_properties.push_str("</w:rPr>");
    format!(
        "<w:p><w:r>{run_properties}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        escape(text)
    )
}

fn table_xml(rows: &[Vec<String>]) -> String {
    let mut xml = String::from(
        r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="999999"/><w:left w:val="single" w:sz="4" w:color="999999"/><w:bottom w:val="single" w:sz="4" w:color="999999"/><w:right w:val="single" w:sz="4" w:color="999999"/><w:insideH w:val="single" w:sz="4" w:color="999999"/><w:insideV w:val="single" w:sz="4" w:color="999999"/></w:tblBorders></w:tblPr>"#,
    );
    for row in rows {
        xml.push_str("<w:tr>");
        for cell in row {
            xml.push_str(r#"<w:tc><w:tcPr><w:tcW w:w="0" w:type="auto"/></w:tcPr>"#);
            xml.push_str(&paragraph(cell, 0, false, false));
            xml.push_str("</w:tc>");
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl><w:p/>");
    xml
}

fn image_paragraph(
    relationship: &str,
    drawing_id: u32,
    cx: i64,
    cy: i64,
    caption: Option<&str>,
    index: usize,
) -> String {
    let description = escape(caption.unwrap_or("deskctl image"));
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{drawing_id}" name="Picture {index}" descr="{description}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{drawing_id}" name="image{index}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{relationship}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

fn extent(width_px: u32, height_px: u32, width_cm: Option<f64>) -> (i64, i64) {
    let natural_width = f64::from(width_px) * EMU_PER_PX;
    let natural_height = f64::from(height_px) * EMU_PER_PX;
    let max_width = width_cm.unwrap_or(DEFAULT_MAX_WIDTH_CM).clamp(1.0, 18.0) * EMU_PER_CM;
    let scale = if natural_width > max_width {
        max_width / natural_width
    } else {
        1.0
    };
    (
        (natural_width * scale) as i64,
        (natural_height * scale) as i64,
    )
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

fn internal(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::Internal, message)
}

/// 读取 `.docx` 的纯文本结构。
#[derive(Debug, Clone)]
pub struct DocxContent {
    pub paragraphs: Vec<String>,
    pub tables: Vec<Vec<Vec<String>>>,
    pub images: usize,
    pub headers: Vec<String>,
    pub footers: Vec<String>,
    pub truncated: bool,
}

/// 解析 `word/document.xml`：段落文本、表格单元格与内嵌图片数量（有上限防爆）。
pub fn read_document(path: &Path) -> Result<DocxContent, ToolError> {
    let file = File::open(path).map_err(|error| internal(format!("open document: {error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| invalid(format!("not a zip package: {error}")))?;
    let images = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .filter(|name| name.starts_with("word/media/"))
        .count();
    let auxiliary: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .filter(|name| name.starts_with("word/header") || name.starts_with("word/footer"))
        .collect();
    let mut headers = Vec::new();
    let mut footers = Vec::new();
    for name in auxiliary {
        let mut xml = String::new();
        if let Ok(mut entry) = archive.by_name(&name) {
            if entry.read_to_string(&mut xml).is_ok() {
                let texts = collect_part_paragraphs(&xml);
                if name.contains("header") {
                    headers.extend(texts);
                } else {
                    footers.extend(texts);
                }
            }
        }
    }
    let mut document = String::new();
    {
        let mut entry = archive
            .by_name("word/document.xml")
            .map_err(|_| invalid("not a .docx package (word/document.xml missing)"))?;
        entry
            .read_to_string(&mut document)
            .map_err(|error| internal(format!("read document xml: {error}")))?;
    }

    let mut reader = quick_xml::Reader::from_str(&document);
    let mut paragraphs: Vec<String> = Vec::new();
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_depth = 0usize;
    let mut row: Option<Vec<String>> = None;
    let mut cell: Option<String> = None;
    let mut paragraph_text: Option<String> = None;
    let mut capturing = false;
    let mut truncated = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| invalid(format!("parse document xml: {error}")))?;
        match event {
            Event::Start(start) => match start.local_name().as_ref() {
                b"tbl" => {
                    table_depth += 1;
                    table_rows.clear();
                }
                b"tr" => row = Some(Vec::new()),
                b"tc" => cell = Some(String::new()),
                b"t" => capturing = true,
                b"p" => paragraph_text = Some(String::new()),
                _ => {}
            },
            Event::Text(text) => {
                if capturing {
                    let value = text.unescape().unwrap_or_default();
                    if let Some(cell_text) = cell.as_mut() {
                        cell_text.push_str(&value);
                    } else if let Some(paragraph) = paragraph_text.as_mut() {
                        paragraph.push_str(&value);
                    }
                }
            }
            Event::End(end) => match end.local_name().as_ref() {
                b"t" => capturing = false,
                b"tc" => {
                    if let (Some(cell_text), Some(row)) = (cell.take(), row.as_mut()) {
                        if row.len() < 100 {
                            row.push(cell_text.trim().to_string());
                        } else {
                            truncated = true;
                        }
                    }
                    paragraph_text = None;
                }
                b"tr" => {
                    if let Some(completed) = row.take() {
                        if table_rows.len() < 500 {
                            table_rows.push(completed);
                        } else {
                            truncated = true;
                        }
                    }
                }
                b"tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        if tables.len() < 100 {
                            tables.push(std::mem::take(&mut table_rows));
                        } else {
                            truncated = true;
                        }
                    }
                }
                b"p" => {
                    if let Some(text) = paragraph_text.take() {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() && table_depth == 0 && cell.is_none() {
                            if paragraphs.len() < 2000 {
                                paragraphs.push(trimmed.to_string());
                            } else {
                                truncated = true;
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(DocxContent {
        paragraphs,
        tables,
        images,
        headers,
        footers,
        truncated,
    })
}

/// 解析页眉/页脚部件：按段落收集 `w:t` 文本；`PAGE` 域输出为 `{PAGE}` 占位。
fn collect_part_paragraphs(xml: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut paragraphs = Vec::new();
    let mut current: Option<String> = None;
    let mut capturing_text = false;
    let mut capturing_instr = false;
    let mut in_field = false;
    let mut field_marked = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
                match start.local_name().as_ref() {
                    b"p" => {
                        current = Some(String::new());
                        in_field = false;
                        field_marked = false;
                    }
                    b"t" => capturing_text = true,
                    b"instrText" => capturing_instr = true,
                    b"fldChar" => {
                        if let Some(kind) = attribute_value(&start, b"fldCharType") {
                            match kind.as_str() {
                                "begin" => {
                                    in_field = true;
                                    field_marked = false;
                                }
                                "end" => in_field = false,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                let value = text.unescape().unwrap_or_default();
                if capturing_instr {
                    if value.contains("PAGE") && !field_marked {
                        if let Some(paragraph) = current.as_mut() {
                            paragraph.push_str("{PAGE}");
                            field_marked = true;
                        }
                    }
                } else if capturing_text && !in_field {
                    if let Some(paragraph) = current.as_mut() {
                        paragraph.push_str(&value);
                    }
                }
            }
            Ok(Event::End(end)) => match end.local_name().as_ref() {
                b"t" => capturing_text = false,
                b"instrText" => capturing_instr = false,
                b"p" => {
                    if let Some(text) = current.take() {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() && paragraphs.len() < 200 {
                            paragraphs.push(trimmed.to_string());
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    paragraphs
}

fn attribute_value(element: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(attribute.value.as_ref()).to_string())
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message)
}

/// 供测试使用的 1x1 PNG。
#[cfg(test)]
pub(crate) fn tiny_png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode png");
    cursor.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_minimal_valid_package() {
        let directory = std::env::temp_dir().join(format!("docx-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("report.docx");
        let png = crate::docx::tiny_png();
        let blocks = vec![
            Block::Title("Deskctl 报表".to_string()),
            Block::Heading("截图".to_string()),
            Block::Paragraph("含 <标签> & 转义".to_string()),
            Block::Image {
                png,
                width_px: 320,
                height_px: 200,
                caption: Some("图 1".to_string()),
                width_cm: Some(10.0),
            },
        ];
        let bytes =
            write_document(&path, &blocks, &DocumentOptions::default()).expect("write docx");
        assert!(bytes > 0);
        let file = File::open(&path).expect("open docx");
        let mut archive = zip::ZipArchive::new(file).expect("docx zip");
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_string())
            .collect();
        names.sort();
        assert!(names.contains(&"[Content_Types].xml".to_string()));
        assert!(names.contains(&"word/document.xml".to_string()));
        assert!(names.contains(&"word/media/image1.png".to_string()));
        assert!(names.contains(&"word/_rels/document.xml.rels".to_string()));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn escapes_xml_text() {
        assert_eq!(escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn round_trips_paragraphs_tables_and_images() {
        let directory = std::env::temp_dir().join(format!("docx-rt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("rt.docx");
        let blocks = vec![
            Block::Title("标题".to_string()),
            Block::Paragraph("第一段 <转义>".to_string()),
            Block::Table(vec![
                vec!["CPU".to_string(), "1.0%".to_string()],
                vec!["内存".to_string(), "6.5%".to_string()],
            ]),
            Block::Image {
                png: tiny_png(),
                width_px: 4,
                height_px: 4,
                caption: None,
                width_cm: None,
            },
        ];
        write_document(&path, &blocks, &DocumentOptions::default()).expect("write docx");
        let content = read_document(&path).expect("read docx");
        assert!(content.paragraphs.iter().any(|line| line == "标题"));
        assert!(content
            .paragraphs
            .iter()
            .any(|line| line == "第一段 <转义>"));
        assert_eq!(content.tables.len(), 1);
        assert_eq!(content.tables[0][1][0], "内存");
        assert_eq!(content.images, 1);
        assert!(!content.truncated);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn writes_and_reads_header_footer_and_page_numbers() {
        let directory = std::env::temp_dir().join(format!("docx-hf-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("hf.docx");
        let blocks = vec![Block::Paragraph("正文".to_string())];
        let options = DocumentOptions {
            header: Some("SAP 巡检报告".to_string()),
            footer: Some("第".to_string()),
            page_numbers: true,
        };
        write_document(&path, &blocks, &options).expect("write docx");
        let file = File::open(&path).expect("open docx");
        let mut archive = zip::ZipArchive::new(file).expect("docx zip");
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_string())
            .collect();
        assert!(names.contains(&"word/header1.xml".to_string()));
        assert!(names.contains(&"word/footer1.xml".to_string()));
        let content = read_document(&path).expect("read docx");
        assert_eq!(content.headers, vec!["SAP 巡检报告".to_string()]);
        assert!(content.footers.iter().any(|line| line.contains("{PAGE}")));
        let _ = std::fs::remove_dir_all(directory);
    }
}
