use crate::config;
use serde_json::{json, Value};
use std::path::Path;

const OCR_MODELS: &[&str] = &[
    "ch_PP-OCRv4_det.onnx",
    "ch_PP-OCRv4_rec.onnx",
    "ch_PP-OCR_keys_v1.txt",
];
const YOLO_MODEL: &str = "icon_detect.onnx";

fn file_check(dir: Option<&Path>, name: &str) -> Value {
    let path = dir.map(|dir| dir.join(name));
    json!({
        "name": name,
        "path": path.as_ref().map(|p| p.display().to_string()),
        "present": path.as_ref().is_some_and(|p| p.is_file()),
    })
}

pub fn report() -> Value {
    let models = config::models_dir();
    let model_dir = models.as_deref();
    let model_files: Vec<Value> = OCR_MODELS
        .iter()
        .map(|name| file_check(model_dir, name))
        .chain(std::iter::once(file_check(model_dir, YOLO_MODEL)))
        .collect();

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let onnx_runtime = [
        "onnxruntime.dll",
        "libonnxruntime.so",
        "libonnxruntime.dylib",
    ]
    .iter()
    .map(|name| file_check(exe_dir.as_deref(), name))
    .collect::<Vec<_>>();

    json!({
        "ok": true,
        "server": {
            "name": "deskctl",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "windows": cfg!(windows),
            "supported_first_phase": cfg!(windows),
        },
        "paths": {
            "config_dir": config::config_dir().display().to_string(),
            "data_dir": config::data_dir().display().to_string(),
            "token_file": config::token_path().display().to_string(),
            "socket": config::default_socket_name(),
            "models_dir": models.as_ref().map(|p| p.display().to_string()),
        },
        "models": {
            "dir_present": models.is_some(),
            "files": model_files,
        },
        "onnx_runtime": {
            "search_dir": exe_dir.as_ref().map(|p| p.display().to_string()),
            "files": onnx_runtime,
        },
    })
}
