//! YOLO icon detection using the bundled `icon_detect.onnx` model.

use std::sync::Mutex;

use deskctl_protocol::{ErrorCode, ToolError};
use image::imageops::FilterType;
use ndarray::Array4;
use serde::Serialize;

use crate::models::YOLO_MODEL;
use crate::session::ScreenRect;

const INPUT_SIZE: u32 = 640;
const CONFIDENCE_THRESHOLD: f32 = 0.25;
const NMS_IOU_THRESHOLD: f32 = 0.45;

#[derive(Debug, Clone, Serialize)]
pub struct IconDetection {
    pub rect: ScreenRect,
    pub confidence: f32,
}

pub struct YoloDetector {
    models_dir: std::path::PathBuf,
    session: Mutex<Option<ort::session::Session>>,
}

impl YoloDetector {
    pub fn new(models_dir: std::path::PathBuf) -> Self {
        Self {
            models_dir,
            session: Mutex::new(None),
        }
    }

    pub fn available(&self) -> bool {
        self.models_dir.join(YOLO_MODEL).is_file()
    }

    pub fn detect(&self, image: &image::RgbaImage) -> Result<Vec<IconDetection>, ToolError> {
        if !self.available() {
            return Err(ToolError::new(
                ErrorCode::ModelUnavailable,
                format!(
                    "YOLO model is missing from {}; run scripts/fetch-models.ps1",
                    self.models_dir.display()
                ),
            )
            .retryable(false));
        }
        let input = preprocess(image)?;
        let mut guard = self
            .session
            .lock()
            .map_err(|_| ToolError::new(ErrorCode::Internal, "YOLO session poisoned"))?;
        if guard.is_none() {
            let runtime = crate::config::onnx_runtime_path().ok_or_else(|| {
                ToolError::new(
                    ErrorCode::ModelUnavailable,
                    "ONNX Runtime was not found; run scripts/fetch-runtime.ps1",
                )
            })?;
            ort::init_from(&runtime)
                .map_err(|error| {
                    ToolError::new(
                        ErrorCode::ModelUnavailable,
                        format!("load ONNX Runtime {}: {error}", runtime.display()),
                    )
                })?
                .commit();
            let session = ort::session::Session::builder()
                .map_err(|error| {
                    ToolError::new(ErrorCode::ModelUnavailable, error.to_string())
                })?
                .commit_from_file(self.models_dir.join(YOLO_MODEL))
                .map_err(|error| {
                    ToolError::new(
                        ErrorCode::ModelUnavailable,
                        format!("load {YOLO_MODEL}: {error}"),
                    )
                })?;
            *guard = Some(session);
        }
        let session = guard.as_mut().expect("YOLO session initialized");
        let input_name = session.inputs()[0].name().to_string();
        let output_name = session.outputs()[0].name().to_string();
        let tensor = ort::value::TensorRef::from_array_view(input.view()).map_err(|error| {
            ToolError::new(ErrorCode::Internal, format!("create YOLO tensor: {error}"))
        })?;
        let outputs = session
            .run(ort::inputs![input_name.as_str() => tensor])
            .map_err(|error| ToolError::new(ErrorCode::Internal, format!("YOLO inference: {error}")))?;
        let output = outputs.get(output_name.as_str()).ok_or_else(|| {
            ToolError::new(ErrorCode::Internal, "YOLO output tensor is missing")
        })?;
        let view = output.try_extract_array::<f32>().map_err(|error| {
            ToolError::new(ErrorCode::Internal, format!("extract YOLO output: {error}"))
        })?;
        let shape = view.shape();
        if shape.len() < 3 || shape[1] < 5 {
            return Err(ToolError::new(
                ErrorCode::Internal,
                format!("unexpected YOLO output shape {shape:?}"),
            ));
        }
        let width = image.width() as f32;
        let height = image.height() as f32;
        let scale_x = width / INPUT_SIZE as f32;
        let scale_y = height / INPUT_SIZE as f32;
        let mut detections = Vec::new();
        for index in 0..shape[2] {
            let confidence = view[[0, 4, index]];
            if confidence < CONFIDENCE_THRESHOLD {
                continue;
            }
            let center_x = view[[0, 0, index]] * scale_x;
            let center_y = view[[0, 1, index]] * scale_y;
            let box_width = view[[0, 2, index]] * scale_x;
            let box_height = view[[0, 3, index]] * scale_y;
            let x = (center_x - box_width / 2.0).max(0.0) as i32;
            let y = (center_y - box_height / 2.0).max(0.0) as i32;
            let w = box_width.min(width - x as f32).max(1.0) as u32;
            let h = box_height.min(height - y as f32).max(1.0) as u32;
            detections.push(IconDetection {
                rect: ScreenRect {
                    x,
                    y,
                    width: w,
                    height: h,
                },
                confidence,
            });
        }
        Ok(nms(detections, NMS_IOU_THRESHOLD))
    }
}

fn preprocess(image: &image::RgbaImage) -> Result<Array4<f32>, ToolError> {
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    let resized = image::imageops::resize(&rgb, INPUT_SIZE, INPUT_SIZE, FilterType::Lanczos3);
    let (width, height) = (resized.width() as usize, resized.height() as usize);
    let raw = resized.into_raw();
    let mut data = vec![0.0f32; 3 * height * width];
    for y in 0..height {
        for x in 0..width {
            let source = (y * width + x) * 3;
            let offset = y * width + x;
            data[offset] = raw[source] as f32 / 255.0;
            data[height * width + offset] = raw[source + 1] as f32 / 255.0;
            data[2 * height * width + offset] = raw[source + 2] as f32 / 255.0;
        }
    }
    Array4::from_shape_vec((1, 3, height, width), data)
        .map_err(|error| ToolError::new(ErrorCode::Internal, format!("YOLO ndarray: {error}")))
}

fn nms(detections: Vec<IconDetection>, threshold: f32) -> Vec<IconDetection> {
    let mut sorted = detections;
    sorted.sort_by(|left, right| {
        right
            .confidence
            .partial_cmp(&left.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<IconDetection> = Vec::new();
    for candidate in sorted {
        if kept
            .iter()
            .all(|existing| iou(&candidate.rect, &existing.rect) <= threshold)
        {
            kept.push(candidate);
        }
    }
    kept
}

fn iou(left: &ScreenRect, right: &ScreenRect) -> f32 {
    let ax1 = left.x as f32;
    let ay1 = left.y as f32;
    let ax2 = ax1 + left.width as f32;
    let ay2 = ay1 + left.height as f32;
    let bx1 = right.x as f32;
    let by1 = right.y as f32;
    let bx2 = bx1 + right.width as f32;
    let by2 = by1 + right.height as f32;
    let intersection_width = (ax2.min(bx2) - ax1.max(bx1)).max(0.0);
    let intersection_height = (ay2.min(by2) - ay1.max(by1)).max(0.0);
    let intersection = intersection_width * intersection_height;
    let union = left.width as f32 * left.height as f32
        + right.width as f32 * right.height as f32
        - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}
