use deskctl_protocol::{ErrorCode, ToolError};
use serde::Serialize;

use crate::session::ScreenRect;

#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub dr: u8,
    pub dg: u8,
    pub db: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindHit {
    pub found: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

pub fn parse_color(value: &str) -> Result<Rgb, ToolError> {
    let value = value.trim();
    if let Some((base, delta)) = value.split_once('-') {
        let base = parse_rgb(base)?;
        let delta = parse_rgb(delta)?;
        return Ok(Rgb {
            r: base.r,
            g: base.g,
            b: base.b,
            dr: delta.r,
            dg: delta.g,
            db: delta.b,
        });
    }
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() >= 3 {
        let r = parse_u8(parts[0])?;
        let g = parse_u8(parts[1])?;
        let b = parse_u8(parts[2])?;
        let dr = parts.get(3).map_or(Ok(0), |value| parse_u8(value))?;
        let dg = parts.get(4).map_or(Ok(0), |value| parse_u8(value))?;
        let db = parts.get(5).map_or(Ok(0), |value| parse_u8(value))?;
        return Ok(Rgb { r, g, b, dr, dg, db });
    }
    let hex = value.trim_start_matches('#');
    if hex.len() == 6 {
        let color = parse_rgb(&format!(
            "{},{},{}",
            u8::from_str_radix(&hex[0..2], 16).map_err(|_| invalid_color(value))?,
            u8::from_str_radix(&hex[2..4], 16).map_err(|_| invalid_color(value))?,
            u8::from_str_radix(&hex[4..6], 16).map_err(|_| invalid_color(value))?
        ))?;
        return Ok(color);
    }
    Err(invalid_color(value))
}

pub fn find_color(
    image: &image::RgbaImage,
    origin: ScreenRect,
    color: Rgb,
    direction: &str,
) -> Option<(i32, i32)> {
    let coordinates = scan_coordinates(image.width(), image.height(), direction);
    for (x, y) in coordinates {
        if pixel_matches(image.get_pixel(x, y), color) {
            return Some((
                origin.x.saturating_add(x as i32),
                origin.y.saturating_add(y as i32),
            ));
        }
    }
    None
}

pub fn find_image(
    image: &image::RgbaImage,
    origin: ScreenRect,
    template: &image::RgbaImage,
    threshold: f32,
) -> FindHit {
    let (tw, th) = (template.width(), template.height());
    if tw == 0 || th == 0 || tw > image.width() || th > image.height() {
        return FindHit {
            found: false,
            x: origin.x,
            y: origin.y,
            width: tw,
            height: th,
            confidence: 0.0,
            diagnostic: Some("template is empty or larger than the captured image".into()),
        };
    }
    let step = if tw >= 64 || th >= 64 { 3 } else { 1 };
    let mut best = (f64::MAX, 0u32, 0u32);
    let template_pixels = template.as_raw();
    let image_pixels = image.as_raw();
    let stride = image.width() as usize * 4;
    let mut y = 0;
    while y + th <= image.height() {
        let mut x = 0;
        while x + tw <= image.width() {
            let diff = window_diff(
                image_pixels,
                stride,
                template_pixels,
                tw as usize,
                th as usize,
                x as usize,
                y as usize,
                best.0,
            );
            if diff < best.0 {
                best = (diff, x, y);
            }
            x += step;
        }
        y += step;
    }
    let confidence = (1.0 - best.0 / 96.0).clamp(0.0, 1.0) as f32;
    FindHit {
        found: confidence >= threshold,
        x: origin.x.saturating_add(best.1 as i32),
        y: origin.y.saturating_add(best.2 as i32),
        width: tw,
        height: th,
        confidence,
        diagnostic: (confidence < threshold).then(|| "best candidate did not reach threshold".into()),
    }
}

fn scan_coordinates(width: u32, height: u32, direction: &str) -> Vec<(u32, u32)> {
    let mut coordinates = Vec::with_capacity((width * height) as usize);
    match direction {
        "left_bottom" => {
            for y in (0..height).rev() {
                for x in 0..width {
                    coordinates.push((x, y));
                }
            }
        }
        "right_top" => {
            for y in 0..height {
                for x in (0..width).rev() {
                    coordinates.push((x, y));
                }
            }
        }
        "right_bottom" => {
            for y in (0..height).rev() {
                for x in (0..width).rev() {
                    coordinates.push((x, y));
                }
            }
        }
        "column_left_top" | "column_right_top" | "column_left_bottom" | "column_right_bottom" => {
            let reverse_x = direction.contains("right");
            let reverse_y = direction.contains("bottom");
            for x in 0..width {
                let actual_x = if reverse_x { width - 1 - x } else { x };
                for y in 0..height {
                    let actual_y = if reverse_y { height - 1 - y } else { y };
                    coordinates.push((actual_x, actual_y));
                }
            }
        }
        "center" => {
            let (cx, cy) = (width / 2, height / 2);
            let max = width.max(height) / 2 + 1;
            for radius in 0..=max {
                for y in cy.saturating_sub(radius)..=(cy + radius).min(height.saturating_sub(1)) {
                    for x in cx.saturating_sub(radius)..=(cx + radius).min(width.saturating_sub(1)) {
                        if x == cx.saturating_sub(radius)
                            || x == (cx + radius).min(width - 1)
                            || y == cy.saturating_sub(radius)
                            || y == (cy + radius).min(height - 1)
                        {
                            coordinates.push((x, y));
                        }
                    }
                }
            }
        }
        _ => {
            for y in 0..height {
                for x in 0..width {
                    coordinates.push((x, y));
                }
            }
        }
    }
    coordinates
}

fn pixel_matches(pixel: &image::Rgba<u8>, color: Rgb) -> bool {
    pixel[0].abs_diff(color.r) <= color.dr
        && pixel[1].abs_diff(color.g) <= color.dg
        && pixel[2].abs_diff(color.b) <= color.db
}

fn window_diff(
    image: &[u8],
    stride: usize,
    template: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    early_stop: f64,
) -> f64 {
    let mut sum = 0u64;
    for row in 0..height {
        let image_offset = (y + row) * stride + x * 4;
        let template_offset = row * width * 4;
        for column in 0..width {
            let io = image_offset + column * 4;
            let to = template_offset + column * 4;
            sum += u64::from(image[io].abs_diff(template[to]));
            sum += u64::from(image[io + 1].abs_diff(template[to + 1]));
            sum += u64::from(image[io + 2].abs_diff(template[to + 2]));
            if sum as f64 > early_stop * (width * height * 3) as f64 {
                return f64::MAX;
            }
        }
    }
    sum as f64 / (width * height * 3) as f64
}

fn parse_u8(value: &str) -> Result<u8, ToolError> {
    value.parse::<u8>().map_err(|_| invalid_color(value))
}

fn parse_rgb(value: &str) -> Result<Rgb, ToolError> {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err(invalid_color(value));
    }
    Ok(Rgb {
        r: parse_u8(parts[0])?,
        g: parse_u8(parts[1])?,
        b: parse_u8(parts[2])?,
        dr: 0,
        dg: 0,
        db: 0,
    })
}

fn invalid_color(value: &str) -> ToolError {
    ToolError::new(
        ErrorCode::InvalidParams,
        format!("invalid color {value:?}; use R,G,B, #RRGGBB or R,G,B,Dr,Dg,Db"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_formats_parse() {
        assert_eq!(parse_color("59,130,246").unwrap().r, 59);
        let hex = parse_color("#3B82F6").unwrap();
        assert_eq!((hex.r, hex.g, hex.b), (59, 130, 246));
        let delta = parse_color("10,20,30,1,2,3").unwrap();
        assert_eq!((delta.dr, delta.dg, delta.db), (1, 2, 3));
    }

    #[test]
    fn exact_image_is_found() {
        let template = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut screen = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        image::imageops::overlay(&mut screen, &template, 8, 7);
        let hit = find_image(
            &screen,
            ScreenRect {
                x: 100,
                y: 200,
                width: 20,
                height: 20,
            },
            &template,
            0.9,
        );
        assert!(hit.found);
        assert_eq!((hit.x, hit.y), (108, 207));
    }
}

