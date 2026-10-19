use deskctl_protocol::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Mutex;

use crate::models::ModelManager;
use crate::session::ScreenRect;

pub mod locate;
mod paddle_ocr;
mod yolo;

const CACHE_CAPACITY: usize = 16;

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
    cache: Mutex<VecDeque<CacheEntry>>,
}

struct CacheEntry {
    key: [u8; 32],
    text_only: bool,
    elements: Vec<VisionElement>,
}

impl VisionEngine {
    pub fn new(models: ModelManager) -> Self {
        let yolo = yolo::YoloDetector::new(models.dir().to_path_buf());
        Self {
            models,
            yolo,
            cache: Mutex::new(VecDeque::new()),
        }
    }

    /// Cached entry point for `desktop_perceive`; returns whether the result
    /// came from the in-process cache.
    pub fn perceive_cached(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
    ) -> Result<(Vec<VisionElement>, bool), ToolError> {
        self.perceive_with_cache(image, screen_rect, false)
    }

    /// Cached OCR-only entry point for `desktop_perceive text_only=true`.
    pub fn perceive_text_cached(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
    ) -> Result<(Vec<VisionElement>, bool), ToolError> {
        self.perceive_with_cache(image, screen_rect, true)
    }

    fn perceive_with_cache(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
        text_only: bool,
    ) -> Result<(Vec<VisionElement>, bool), ToolError> {
        let key = Self::image_key(image);
        if let Ok(mut cache) = self.cache.lock() {
            if let Some(index) = cache
                .iter()
                .position(|entry| entry.text_only == text_only && entry.key == key)
            {
                if let Some(entry) = cache.remove(index) {
                    let elements = entry.elements.clone();
                    cache.push_back(entry);
                    return Ok((
                        Self::with_screen_centers(elements, screen_rect, image),
                        true,
                    ));
                }
            }
        }

        let elements = if text_only {
            self.perceive_text(image, screen_rect)?
        } else {
            self.perceive(image, screen_rect)?
        };
        let mut cached_elements = elements.clone();
        for element in &mut cached_elements {
            element.screen_center = None;
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.push_back(CacheEntry {
                key,
                text_only,
                elements: cached_elements,
            });
            while cache.len() > CACHE_CAPACITY {
                cache.pop_front();
            }
        }
        Ok((elements, false))
    }

    fn image_key(image: &image::RgbaImage) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(image.width().to_le_bytes());
        hasher.update(image.height().to_le_bytes());
        hasher.update(image.as_raw());
        let digest = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        key
    }

    fn with_screen_centers(
        mut elements: Vec<VisionElement>,
        screen_rect: Option<ScreenRect>,
        image: &image::RgbaImage,
    ) -> Vec<VisionElement> {
        let origin = screen_rect.unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        });
        for element in &mut elements {
            element.screen_center = Some((
                origin.x + element.rect.x + element.rect.width as i32 / 2,
                origin.y + element.rect.y + element.rect.height as i32 / 2,
            ));
        }
        elements
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
        let mut elements = Vec::new();
        if self.models.ocr_available() {
            elements.extend(self.ocr_elements(image, screen_rect)?);
        }
        if self.yolo.available() {
            let origin = screen_rect.unwrap_or(ScreenRect {
                x: 0,
                y: 0,
                width: image.width(),
                height: image.height(),
            });
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
        Ok(Self::finish(elements))
    }

    /// OCR-only path used by `desktop_perceive text_only=true`.
    pub fn perceive_text(
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
        Ok(Self::finish(self.ocr_elements(image, screen_rect)?))
    }

    fn ocr_elements(
        &self,
        image: &image::RgbaImage,
        screen_rect: Option<ScreenRect>,
    ) -> Result<Vec<VisionElement>, ToolError> {
        let origin = screen_rect.unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        });
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
        let blocks = paddle_ocr::PaddleOcr::with_engine(|engine| engine.ocr_with_boxes_image(&rgb))
            .map_err(|error| {
                ToolError::new(
                    ErrorCode::ModelUnavailable,
                    format!("PaddleOCR failed: {error}"),
                )
                .retryable(false)
            })?;
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
                confidence: block.confidence,
                screen_center: Some((
                    origin.x + block.x + block.w / 2,
                    origin.y + block.y + block.h / 2,
                )),
            })
            .collect())
    }

    fn finish(mut elements: Vec<VisionElement>) -> Vec<VisionElement> {
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
        elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn image_key_is_stable_and_pixel_sensitive() {
        let first = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 4]));
        let same = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 4]));
        let different = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 5]));
        assert_eq!(
            VisionEngine::image_key(&first),
            VisionEngine::image_key(&same)
        );
        assert_ne!(
            VisionEngine::image_key(&first),
            VisionEngine::image_key(&different)
        );
    }

    #[test]
    fn with_screen_centers_applies_origin() {
        let image = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0]));
        let elements = vec![VisionElement {
            kind: "text".to_string(),
            text: "x".to_string(),
            rect: ScreenRect {
                x: 10,
                y: 20,
                width: 4,
                height: 6,
            },
            confidence: 0.9,
            screen_center: None,
        }];
        let shifted = VisionEngine::with_screen_centers(
            elements,
            Some(ScreenRect {
                x: 100,
                y: 200,
                width: 10,
                height: 10,
            }),
            &image,
        );
        assert_eq!(shifted[0].screen_center, Some((112, 223)));
    }
}
