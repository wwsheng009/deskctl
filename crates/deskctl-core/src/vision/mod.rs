use deskctl_protocol::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};

use crate::models::ModelManager;
use crate::session::ScreenRect;

pub mod locate;
mod paddle_ocr;
mod yolo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionElement {
    pub kind: String,
    pub text: String,
    pub rect: ScreenRect,
    pub confidence: f32,
    pub screen_center: Option<(i32, i32)>,
}

pub struct VisionEngine {
    models: ModelManager,
    yolo: yolo::YoloDetector,
}

impl VisionEngine {
    pub fn new(models: ModelManager) -> Self {
        let yolo = yolo::YoloDetector::new(models.dir().to_path_buf());
        Self { models, yolo }
    }

    pub fn perceive(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
    ) -> Result<Vec<VisionElement>, ToolError> {
        if !self.models.ocr_available() && !self.yolo.available() {
            return Err(ToolError::new(
                ErrorCode::ModelUnavailable,
                format!(
                    "OCR/YOLO models are missing from {}; run scripts/fetch-models.ps1",
                    self.models.dir().display()
                ),
            )
            .retryable(false));
        }
        let origin = screen_rect.unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        });
        let mut elements = Vec::new();
        if self.models.ocr_available() {
            let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
            let blocks =
                paddle_ocr::PaddleOcr::with_engine(|engine| engine.ocr_with_boxes_image(&rgb))
                    .map_err(|error| {
                        ToolError::new(
                            ErrorCode::ModelUnavailable,
                            format!("PaddleOCR failed: {error}"),
                        )
                        .retryable(false)
                    })?;
            elements.extend(blocks.into_iter().map(|block| VisionElement {
                kind: "text".to_string(),
                text: block.text,
                rect: ScreenRect {
                    x: block.x,
                    y: block.y,
                    width: block.w.max(1) as u32,
                    height: block.h.max(1) as u32,
                },
                confidence: 0.9,
                screen_center: Some((
                    origin.x + block.x + block.w / 2,
                    origin.y + block.y + block.h / 2,
                )),
            }));
        }
        if self.yolo.available() {
            let icons = self.yolo.detect(image)?;
            elements.extend(icons.into_iter().map(|icon| VisionElement {
                kind: "icon".to_string(),
                text: String::new(),
                rect: icon.rect,
                confidence: icon.confidence,
                screen_center: Some((
                    origin.x + icon.rect.x + icon.rect.width as i32 / 2,
                    origin.y + icon.rect.y + icon.rect.height as i32 / 2,
                )),
            }));
        }
        elements.sort_by(|left, right| {
            left.rect
                .y
                .cmp(&right.rect.y)
                .then(left.rect.x.cmp(&right.rect.x))
                .then(left.rect.width.cmp(&right.rect.width))
                .then(left.rect.height.cmp(&right.rect.height))
        });
        elements.dedup_by(|left, right| {
            left.kind == right.kind && left.text == right.text && left.rect == right.rect
        });
        Ok(elements)
    }
}
