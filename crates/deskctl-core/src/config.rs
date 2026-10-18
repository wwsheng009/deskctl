use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use deskctl_protocol::ToolProfile;

use crate::capture_policy::{CapturePolicy, OutputFormat};

#[derive(Debug, Clone)]
pub struct Config {
    pub models_dir: PathBuf,
    pub output_dir: PathBuf,
    pub max_sessions: usize,
    pub profile: ToolProfile,
    pub capture_cache_limit: usize,
    pub observation_ttl: Duration,
    pub session_idle: Duration,
    pub resource_wait: Duration,
    pub capture: CapturePolicy,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            models_dir: models_dir().unwrap_or_else(|| data_dir().join("models")),
            output_dir: data_dir().join("captures"),
            max_sessions: env_usize("DESKCTL_MAX_SESSIONS", 8),
            profile: std::env::var("DESKCTL_PROFILE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            capture_cache_limit: env_usize("DESKCTL_CAPTURE_CACHE_LIMIT", 32),
            observation_ttl: Duration::from_secs(env_u64("DESKCTL_OBSERVATION_TTL_SECS", 120)),
            session_idle: Duration::from_secs(env_u64("DESKCTL_SESSION_IDLE_SECS", 30 * 60)),
            resource_wait: Duration::from_secs(env_u64("DESKCTL_RESOURCE_WAIT_SECS", 30)),
            capture: capture_policy_from_env(),
        }
    }
}

fn capture_policy_from_env() -> CapturePolicy {
    let mut policy = CapturePolicy::default();
    if let Ok(format) = std::env::var("DESKCTL_CAPTURE_FORMAT") {
        if let Ok(format) = OutputFormat::parse(&format) {
            policy.format = format;
        }
    }
    policy.quality =
        env_u64("DESKCTL_CAPTURE_QUALITY", u64::from(policy.quality)).clamp(1, 100) as u8;
    policy.png_compression = env_u64(
        "DESKCTL_CAPTURE_PNG_COMPRESSION",
        u64::from(policy.png_compression),
    )
    .min(9) as u8;
    policy.max_width = std::env::var("DESKCTL_CAPTURE_MAX_WIDTH")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0);
    policy.max_height = std::env::var("DESKCTL_CAPTURE_MAX_HEIGHT")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0);
    policy.max_bytes = std::env::var("DESKCTL_CAPTURE_MAX_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0);
    policy
}

impl Config {
    pub fn ensure_directories(&self) {
        let _ = std::fs::create_dir_all(&self.models_dir);
        let _ = std::fs::create_dir_all(&self.output_dir);
    }

    pub fn is_allowed_image_path(&self, path: &Path) -> bool {
        if std::env::var_os("DESKCTL_ALLOW_ANY_INPUT").is_some() {
            return true;
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        let path = absolute.canonicalize().unwrap_or(absolute);
        let mut roots = vec![
            self.models_dir.clone(),
            self.output_dir.clone(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            std::env::temp_dir(),
        ];
        if let Ok(root) = std::env::var("DESKCTL_ALLOWED_INPUT_ROOT") {
            roots.push(PathBuf::from(root));
        }
        roots.into_iter().any(|root| {
            let root = root.canonicalize().unwrap_or(root);
            path.starts_with(root)
        })
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("deskctl")
}

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(config_dir)
        .join("deskctl")
}

pub fn token_path() -> PathBuf {
    if let Ok(path) = std::env::var("DESKCTL_TOKEN_FILE") {
        return PathBuf::from(path);
    }
    config_dir().join("daemon.token")
}

pub fn default_socket_name() -> String {
    std::env::var("DESKCTL_SOCKET").unwrap_or_else(|_| {
        if cfg!(windows) {
            r"\\.\pipe\deskctl".to_string()
        } else {
            let runtime = std::env::var("XDG_RUNTIME_DIR")
                .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
            PathBuf::from(runtime)
                .join("deskctl.sock")
                .to_string_lossy()
                .into_owned()
        }
    })
}

pub fn models_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("DESKCTL_MODELS_DIR").map(PathBuf::from),
        Some(data_dir().join("models")),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("models"))),
        Some(PathBuf::from("models")),
    ];
    let dirs: Vec<PathBuf> = candidates
        .into_iter()
        .flatten()
        .filter(|path| path.is_dir())
        .collect();
    // A stale or empty directory must not shadow one that actually ships
    // models (for example an empty %LOCALAPPDATA%\deskctl\models sitting in
    // front of the models/ directory next to the executable).
    dirs.iter()
        .find(|path| has_model_files(path))
        .cloned()
        .or_else(|| dirs.into_iter().next())
}

fn has_model_files(dir: &Path) -> bool {
    [
        crate::models::OCR_DET_MODEL,
        crate::models::OCR_REC_MODEL,
        crate::models::YOLO_MODEL,
        crate::models::HASHES_FILE,
    ]
    .iter()
    .any(|name| dir.join(name).is_file())
}

pub fn onnx_runtime_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let name = if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from));
    exe_dir
        .into_iter()
        .chain(models_dir())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}
