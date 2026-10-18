use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const OCR_DET_MODEL: &str = "ch_PP-OCRv4_det.onnx";
pub const OCR_REC_MODEL: &str = "ch_PP-OCRv4_rec.onnx";
pub const OCR_DICT: &str = "ch_PP-OCR_keys_v1.txt";
pub const YOLO_MODEL: &str = "icon_detect.onnx";
pub const ONNX_RUNTIME: &str = "onnxruntime.dll";
pub const HASHES_FILE: &str = "hashes.json";

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub models_dir: String,
    pub ocr_available: bool,
    pub yolo_available: bool,
    pub runtime_available: bool,
    pub hash_validation: String,
    pub missing: Vec<String>,
    pub invalid_hashes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModelManager {
    dir: PathBuf,
}

impl ModelManager {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ocr_model_paths(&self) -> (PathBuf, PathBuf, PathBuf) {
        (
            self.dir.join(OCR_DET_MODEL),
            self.dir.join(OCR_REC_MODEL),
            self.dir.join(OCR_DICT),
        )
    }

    pub fn yolo_model_path(&self) -> PathBuf {
        self.dir.join(YOLO_MODEL)
    }

    pub fn runtime_path(&self) -> PathBuf {
        self.dir.join(ONNX_RUNTIME)
    }

    pub fn ocr_available(&self) -> bool {
        let (det, rec, dict) = self.ocr_model_paths();
        det.is_file() && rec.is_file() && dict.is_file()
    }

    pub fn yolo_available(&self) -> bool {
        self.yolo_model_path().is_file()
    }

    pub fn runtime_available(&self) -> bool {
        let path = self.runtime_path();
        path.is_file()
            || std::env::var_os("ORT_DYLIB_PATH").is_some()
            || std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|parent| parent.join(ONNX_RUNTIME)))
                .is_some_and(|path| path.is_file())
    }

    pub fn status(&self) -> ModelStatus {
        let mut missing = Vec::new();
        for file in [OCR_DET_MODEL, OCR_REC_MODEL, OCR_DICT, YOLO_MODEL] {
            if !self.dir.join(file).is_file() {
                missing.push(file.to_string());
            }
        }
        let (hash_validation, invalid_hashes) = self.validate_hashes();
        ModelStatus {
            models_dir: self.dir.display().to_string(),
            ocr_available: self.ocr_available(),
            yolo_available: self.yolo_available(),
            runtime_available: self.runtime_available(),
            hash_validation,
            missing,
            invalid_hashes,
        }
    }

    fn validate_hashes(&self) -> (String, Vec<String>) {
        let path = self.dir.join(HASHES_FILE);
        if !path.is_file() {
            return ("not_configured".to_string(), Vec::new());
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            return ("unreadable".to_string(), Vec::new());
        };
        let Ok(serde_json::Value::Object(expected)) =
            serde_json::from_str::<serde_json::Value>(&content)
        else {
            return ("invalid".to_string(), Vec::new());
        };
        let mut invalid = Vec::new();
        for (name, hash) in expected {
            let Some(hash) = hash.as_str() else {
                invalid.push(name);
                continue;
            };
            let file = self.dir.join(&name);
            if !file.is_file() {
                continue;
            }
            match file_sha256(&file) {
                Ok(actual) if actual.eq_ignore_ascii_case(hash) => {}
                _ => invalid.push(name),
            }
        }
        (
            if invalid.is_empty() {
                "ok".to_string()
            } else {
                "mismatch".to_string()
            },
            invalid,
        )
    }
}

pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

