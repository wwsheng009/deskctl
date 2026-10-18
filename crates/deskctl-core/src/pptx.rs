//! 极简 PPTX 读取/写出。
//!
//! - 读取：zip + quick-xml 提取每页文本（`a:t`）与图片（`a:blip`）数量。
//! - 写出：最小 OOXML 演示文稿（母版/版式/主题脚手架 + 标题与要点文本框），
//!   PowerPoint / WPS / LibreOffice 可打开。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use deskctl_protocol::{ErrorCode, ToolError};
use quick_xml::events::Event;
use serde_json::{json, Value};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

const MAX_SLIDES: usize = 200;
const MAX_TEXTS_PER_SLIDE: usize = 200;
const SLIDE_ID_BASE: u32 = 256;

/// 读取演示文稿：`{ slide_count, slides: [{ index, texts, images }] }`。
pub fn read_presentation(path: &Path) -> Result<Value, ToolError> {
    let file = File::open(path).map_err(|error| internal(format!("open presentation: {error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| invalid(format!("not a zip package: {error}")))?;
    let mut slide_names: Vec<(usize, String)> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .filter_map(|name| {
            let number: usize = name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse()
                .ok()?;
            Some((number, name))
        })
        .collect();
    slide_names.sort();
    slide_names.truncate(MAX_SLIDES);
    let mut slides = Vec::new();
    for (number, name) in slide_names {
        let mut xml = String::new();
        archive
            .by_name(&name)
            .map_err(|error| internal(format!("read {name}: {error}")))?
            .read_to_string(&mut xml)
            .map_err(|error| internal(format!("read {name}: {error}")))?;
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut texts = Vec::new();
        let mut images = 0usize;
        let mut capturing = false;
        let mut buffer = String::new();
        loop {
            match reader.read_event() {
                Ok(Event::Start(start)) => match start.local_name().as_ref() {
                    b"t" => {
                        capturing = true;
                        buffer.clear();
                    }
                    b"blip" => images += 1,
                    _ => {}
                },
                Ok(Event::Empty(start)) => {
                    if start.local_name().as_ref() == b"blip" {
                        images += 1;
                    }
                }
                Ok(Event::Text(text)) => {
                    if capturing {
                        buffer.push_str(&text.unescape().unwrap_or_default());
                    }
                }
                Ok(Event::End(end)) => {
                    if end.local_name().as_ref() == b"t" {
                        capturing = false;
                        let trimmed = buffer.trim();
                        if !trimmed.is_empty() && texts.len() < MAX_TEXTS_PER_SLIDE {
                            texts.push(trimmed.to_string());
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(invalid(format!("parse {name}: {error}"))),
                _ => {}
            }
        }
        slides.push(json!({ "index": number, "texts": texts, "images": images }));
    }
    let slide_count = slides.len();
    Ok(json!({ "slide_count": slide_count, "slides": slides }))
}

/// 单页图片（PNG 字节 + 像素尺寸）。
#[derive(Debug, Clone)]
pub struct SlideImage {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
}

/// 版式：`Auto` 按内容推断，其余为显式选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlideLayout {
    /// 有图无要点 → 标题+图片；有图有点 → 左文右图；仅要点 → 标题+要点；否则仅标题。
    #[default]
    Auto,
    /// 标题 + 要点（不接受图片）。
    TitleBullets,
    /// 标题 + 整幅图片（不接受要点，需要图片）。
    TitleImage,
    /// 左侧要点 + 右侧图片（需要图片，要点可选）。
    TextImage,
    /// 仅标题（不接受图片与要点）。
    TitleOnly,
}

/// 单页规格：标题 + 要点（每行一个段落）+ 可选图片。
#[derive(Debug, Clone, Default)]
pub struct SlideSpec {
    pub title: Option<String>,
    pub bullets: Vec<String>,
    pub layout: SlideLayout,
    pub image: Option<SlideImage>,
}

const EMU_PER_PX: f64 = 9525.0; // 96 dpi

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveLayout {
    TitleBullets,
    TitleImage,
    TextImage,
    TitleOnly,
}

/// 根据显式版式或内容推断实际版式；不匹配时返回 `invalid_params`。
fn effective_layout(slide: &SlideSpec) -> Result<EffectiveLayout, ToolError> {
    let has_image = slide.image.is_some();
    let has_bullets = slide.bullets.iter().any(|bullet| !bullet.trim().is_empty());
    match slide.layout {
        SlideLayout::Auto => Ok(match (has_image, has_bullets) {
            (false, true) => EffectiveLayout::TitleBullets,
            (false, false) => EffectiveLayout::TitleOnly,
            (true, true) => EffectiveLayout::TextImage,
            (true, false) => EffectiveLayout::TitleImage,
        }),
        SlideLayout::TitleBullets if has_image => {
            Err(invalid("layout 'title_bullets' does not accept an image"))
        }
        SlideLayout::TitleBullets => Ok(EffectiveLayout::TitleBullets),
        SlideLayout::TitleImage if !has_image => {
            Err(invalid("layout 'title_image' requires an image"))
        }
        SlideLayout::TitleImage if has_bullets => Err(invalid(
            "layout 'title_image' ignores bullets; use 'text_image' instead",
        )),
        SlideLayout::TitleImage => Ok(EffectiveLayout::TitleImage),
        SlideLayout::TextImage if !has_image => {
            Err(invalid("layout 'text_image' requires an image"))
        }
        SlideLayout::TextImage => Ok(EffectiveLayout::TextImage),
        SlideLayout::TitleOnly if has_image => {
            Err(invalid("layout 'title_only' does not accept an image"))
        }
        SlideLayout::TitleOnly if has_bullets => {
            Err(invalid("layout 'title_only' does not accept bullets"))
        }
        SlideLayout::TitleOnly => Ok(EffectiveLayout::TitleOnly),
    }
}

/// 写出 `.pptx`，返回字节数。
pub fn write_presentation(path: &Path, slides: &[SlideSpec]) -> Result<u64, ToolError> {
    if slides.is_empty() {
        return Err(invalid("at least one slide is required"));
    }
    if slides.len() > MAX_SLIDES {
        return Err(invalid(format!("too many slides (>{MAX_SLIDES})")));
    }
    for slide in slides {
        effective_layout(slide)?;
        if let Some(image) = &slide.image {
            if image.png.is_empty() || image.width_px == 0 || image.height_px == 0 {
                return Err(invalid(
                    "slide image must be non-empty with positive pixel dimensions",
                ));
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| internal(format!("create output directory: {error}")))?;
    }
    let file =
        File::create(path).map_err(|error| internal(format!("create presentation: {error}")))?;
    let mut zip = ZipWriter::new(file);
    write_entry(
        &mut zip,
        "[Content_Types].xml",
        content_types(slides.len()).as_bytes(),
    )?;
    write_entry(&mut zip, "_rels/.rels", ROOT_RELS.as_bytes())?;
    write_entry(
        &mut zip,
        "ppt/presentation.xml",
        presentation_xml(slides.len()).as_bytes(),
    )?;
    write_entry(
        &mut zip,
        "ppt/_rels/presentation.xml.rels",
        presentation_rels(slides.len()).as_bytes(),
    )?;
    write_entry(
        &mut zip,
        "ppt/slideMasters/slideMaster1.xml",
        SLIDE_MASTER.as_bytes(),
    )?;
    write_entry(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        SLIDE_MASTER_RELS.as_bytes(),
    )?;
    write_entry(
        &mut zip,
        "ppt/slideLayouts/slideLayout1.xml",
        SLIDE_LAYOUT.as_bytes(),
    )?;
    write_entry(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        SLIDE_LAYOUT_RELS.as_bytes(),
    )?;
    write_entry(&mut zip, "ppt/theme/theme1.xml", THEME.as_bytes())?;
    let mut media: Vec<Vec<u8>> = Vec::new();
    for (index, slide) in slides.iter().enumerate() {
        let layout = effective_layout(slide)?;
        let has_image = if let Some(image) = &slide.image {
            media.push(image.png.clone());
            true
        } else {
            false
        };
        write_entry(
            &mut zip,
            &format!("ppt/slides/slide{}.xml", index + 1),
            slide_xml(slide, layout, has_image).as_bytes(),
        )?;
        write_entry(
            &mut zip,
            &format!("ppt/slides/_rels/slide{}.xml.rels", index + 1),
            slide_rels(has_image).as_bytes(),
        )?;
    }
    for (index, png) in media.iter().enumerate() {
        write_entry(&mut zip, &format!("ppt/media/image{}.png", index + 1), png)?;
    }
    zip.finish()
        .map_err(|error| internal(format!("finalize presentation: {error}")))?;
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| internal(format!("read presentation metadata: {error}")))
}
fn content_types(slide_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#,
    );
    for index in 1..=slide_count {
        xml.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
    }
    xml.push_str("</Types>");
    xml
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#;

fn presentation_xml(slide_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>"#,
    );
    for index in 0..slide_count {
        xml.push_str(&format!(
            r#"<p:sldId id="{}" r:id="rId{}"/>"#,
            SLIDE_ID_BASE + index as u32,
            index + 2
        ));
    }
    xml.push_str(
        r#"</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#,
    );
    xml
}

fn presentation_rels(slide_count: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    xml.push_str(
        r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#,
    );
    for index in 0..slide_count {
        xml.push_str(&format!(
            r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
            index + 2,
            index + 1
        ));
    }
    xml.push_str(
        r#"<Relationship Id="rIdTheme" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/></Relationships>"#,
    );
    xml
}

const SLIDE_MASTER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#;

const SLIDE_MASTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#;

const SLIDE_LAYOUT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1"><p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#;

const SLIDE_LAYOUT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#;

fn slide_rels(has_image: bool) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#,
    );
    if has_image {
        xml.push_str(
            r#"<Relationship Id="rIdImg1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>"#,
        );
    }
    xml.push_str("</Relationships>");
    xml
}

const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Deskctl"><a:themeElements><a:clrScheme name="Deskctl"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Deskctl"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Deskctl"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

fn slide_xml(slide: &SlideSpec, layout: EffectiveLayout, has_image: bool) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
    );
    let mut shape_id = 2u32;
    if let Some(title) = slide
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        xml.push_str(&text_shape(
            shape_id,
            "Title",
            838200,
            457200,
            10515600,
            1325563,
            2400,
            true,
            &[title.to_string()],
        ));
        shape_id += 1;
    }
    let bullets: Vec<String> = slide
        .bullets
        .iter()
        .filter(|bullet| !bullet.trim().is_empty())
        .cloned()
        .collect();
    match layout {
        EffectiveLayout::TitleBullets => {
            xml.push_str(&text_shape(
                shape_id, "Body", 838200, 2000000, 10515600, 4000000, 1600, false, &bullets,
            ));
        }
        EffectiveLayout::TextImage => {
            if !bullets.is_empty() {
                xml.push_str(&text_shape(
                    shape_id, "Body", 838200, 2000000, 4500000, 4200000, 1600, false, &bullets,
                ));
                shape_id += 1;
            }
            if let Some(image) = slide.image.as_ref().filter(|_| has_image) {
                xml.push_str(&image_shape(
                    image, shape_id, 6176400, 2000000, 5177400, 4200000,
                ));
            }
        }
        EffectiveLayout::TitleImage => {
            if let Some(image) = slide.image.as_ref().filter(|_| has_image) {
                xml.push_str(&image_shape(
                    image, shape_id, 838200, 2000000, 10515600, 4200000,
                ));
            }
        }
        EffectiveLayout::TitleOnly => {}
    }
    xml.push_str("</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>");
    xml
}

/// 生成图片形状：按原始宽高比等比缩放并居中到给定框内。
fn image_shape(
    image: &SlideImage,
    shape_id: u32,
    x: i64,
    y: i64,
    box_cx: i64,
    box_cy: i64,
) -> String {
    let natural_width = f64::from(image.width_px) * EMU_PER_PX;
    let natural_height = f64::from(image.height_px) * EMU_PER_PX;
    let scale = (box_cx as f64 / natural_width).min(box_cy as f64 / natural_height);
    let cx = (natural_width * scale) as i64;
    let cy = (natural_height * scale) as i64;
    let off_x = x + (box_cx - cx) / 2;
    let off_y = y + (box_cy - cy) / 2;
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="{shape_id}" name="image{shape_id}.png"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg1"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="{off_x}" y="{off_y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#
    )
}

#[allow(clippy::too_many_arguments)]
fn text_shape(
    shape_id: u32,
    name: &str,
    x: i64,
    y: i64,
    cx: i64,
    cy: i64,
    size_half_pt: u32,
    bold: bool,
    paragraphs: &[String],
) -> String {
    let mut xml = format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{shape_id}" name="{name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm></p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/>"#
    );
    for paragraph in paragraphs {
        let bold_attr = if bold { r#" b="1""# } else { "" };
        xml.push_str(&format!(
            r#"<a:p><a:r><a:rPr lang="zh-CN" sz="{size_half_pt}"{bold_attr} dirty="0"/><a:t>{}</a:t></a:r><a:endParaRPr lang="zh-CN" dirty="0"/></a:p>"#,
            escape(paragraph)
        ));
    }
    xml.push_str("</p:txBody></p:sp>");
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
    fn round_trips_titles_and_bullets() {
        let directory = std::env::temp_dir().join(format!("pptx-rt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("rt.pptx");
        let slides = vec![
            SlideSpec {
                title: Some("SAP ST06 报表".to_string()),
                bullets: vec!["CPU 已消耗 1.0%".to_string(), "内存可用 6.5%".to_string()],
                ..Default::default()
            },
            SlideSpec {
                title: Some("第二页".to_string()),
                bullets: vec![],
                ..Default::default()
            },
        ];
        let bytes = write_presentation(&path, &slides).expect("write pptx");
        assert!(bytes > 0);
        let read = read_presentation(&path).expect("read pptx");
        assert_eq!(read["slide_count"], 2);
        assert_eq!(read["slides"][0]["texts"][0], "SAP ST06 报表");
        assert!(read["slides"][0]["texts"]
            .as_array()
            .expect("texts")
            .iter()
            .any(|text| text == "内存可用 6.5%"));
        assert_eq!(read["slides"][1]["texts"][0], "第二页");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn writes_image_slides_and_validates_layouts() {
        let directory = std::env::temp_dir().join(format!("pptx-img-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("img.pptx");
        let image = SlideImage {
            png: crate::docx::tiny_png(),
            width_px: 320,
            height_px: 200,
        };
        let slides = vec![
            SlideSpec {
                title: Some("截图页".to_string()),
                bullets: vec!["左列说明".to_string()],
                layout: SlideLayout::Auto,
                image: Some(image.clone()),
            },
            SlideSpec {
                title: Some("整幅截图".to_string()),
                bullets: vec![],
                layout: SlideLayout::TitleImage,
                image: Some(image),
            },
        ];
        write_presentation(&path, &slides).expect("write pptx");
        let read = read_presentation(&path).expect("read pptx");
        assert_eq!(read["slides"][0]["images"], 1);
        assert_eq!(read["slides"][1]["images"], 1);
        assert!(read["slides"][0]["texts"]
            .as_array()
            .expect("texts")
            .iter()
            .any(|text| text == "左列说明"));
        let file = File::open(&path).expect("open pptx");
        let mut archive = zip::ZipArchive::new(file).expect("zip");
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_string())
            .collect();
        assert!(names.contains(&"ppt/media/image1.png".to_string()));
        assert!(names.contains(&"ppt/media/image2.png".to_string()));
        let bad = vec![SlideSpec {
            title: None,
            bullets: vec!["无图".to_string()],
            layout: SlideLayout::TextImage,
            image: None,
        }];
        assert!(write_presentation(&directory.join("bad.pptx"), &bad).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }
}
