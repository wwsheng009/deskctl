use deskctl_protocol::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};

use crate::models::ModelManager;
use crate::session::ScreenRect;

pub mod locate;
mod paddle_ocr;

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
}

impl VisionEngine {
    pub fn new(models: ModelManager) -> Self {
        Self { models }
    }

    pub fn perceive(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
    ) -> Result<Vec<VisionElement>, ToolError> {
        if !self.models.ocr_available() {
            return Err(ToolError::new(
                ErrorCode::ModelUnavailable,
                format!(
                    "OCR models are missing from {}; run scripts/fetch-models.ps1",
                    self.models.dir().display()
                ),
            )
            .retryable(false));
        }
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
        let blocks = paddle_ocr::PaddleOcr::with_engine(|engine| engine.ocr_with_boxes_image(&rgb))
            .map_err(|error| {
                ToolError::new(
                    ErrorCode::ModelUnavailable,
                    format!("PaddleOCR failed: {error}"),
                )
                .retryable(false)
            })?;
        let origin = screen_rect.unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        });
        Ok(blocks
            .into_iter()
            .map(|block| VisionElement {
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
            })
            .collect())
    }
}
