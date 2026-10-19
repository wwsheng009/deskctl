use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::output;

const SCRIPT: &str = include_str!("../../../scripts/extract-word.ps1");

pub struct DocExtractOptions {
    pub file: PathBuf,
    pub out_dir: PathBuf,
    pub output: Option<PathBuf>,
    pub batch_out: Option<PathBuf>,
    pub compact: bool,
}

/// Extract text and embedded images from a Word document via Word COM.
///
/// This is a CLI helper (not a daemon tool): it keeps the tool contract
/// unchanged and pairs naturally with `deskctl batch` for OCR of the exported
/// images. Requires Windows and Microsoft Word.
pub fn run(options: DocExtractOptions) -> Result<i32, String> {
    if !options.file.is_file() {
        return Err(format!(
            "invalid_params: file '{}' does not exist",
            options.file.display()
        ));
    }
    std::fs::create_dir_all(&options.out_dir).map_err(|error| {
        format!(
            "invalid_params: create output directory '{}': {error}",
            options.out_dir.display()
        )
    })?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let script_path = std::env::temp_dir().join(format!(
        "deskctl-doc-extract-{}-{stamp}.ps1",
        std::process::id()
    ));
    std::fs::write(&script_path, SCRIPT)
        .map_err(|error| format!("write temporary PowerShell script: {error}"))?;
    let result = invoke_powershell(&script_path, &options.file, &options.out_dir);
    let _ = std::fs::remove_file(&script_path);
    let result = result?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        return Err(format!(
            "doc_extract_failed: {}",
            [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }

    let metadata_path = options.out_dir.join("metadata.json");
    let raw = std::fs::read_to_string(&metadata_path).map_err(|error| {
        format!(
            "doc_extract_failed: read '{}': {error}",
            metadata_path.display()
        )
    })?;
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
    let mut metadata: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|error| format!("doc_extract_failed: invalid metadata JSON: {error}"))?;
    if let Some(path) = options.batch_out.as_deref() {
        write_batch_file(path, &metadata)?;
        metadata["batch_path"] = serde_json::json!(path.display().to_string());
    }
    output::print_json_with_options(&metadata, options.compact, options.output.as_deref())?;
    Ok(0)
}

fn write_batch_file(path: &Path, metadata: &serde_json::Value) -> Result<(), String> {
    let images = metadata
        .get("images")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = String::new();
    for image in images {
        let image_path = image.as_str().ok_or_else(|| {
            "doc_extract_failed: metadata images must contain string paths".to_string()
        })?;
        let line = serde_json::json!({
            "tool": "desktop_perceive",
            "arguments": {
                "image_path": image_path,
                "text_only": true
            }
        });
        lines.push_str(&serde_json::to_string(&line).map_err(|error| error.to_string())?);
        lines.push('\n');
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "create batch file directory '{}': {error}",
                    parent.display()
                )
            })?;
        }
    }
    std::fs::write(path, lines)
        .map_err(|error| format!("write batch file '{}': {error}", path.display()))
}

fn invoke_powershell(
    script: &Path,
    file: &Path,
    out_dir: &Path,
) -> Result<std::process::Output, String> {
    let args: Vec<OsString> = vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-File".into(),
        script.as_os_str().to_owned(),
        "-Path".into(),
        file.as_os_str().to_owned(),
        "-OutDir".into(),
        out_dir.as_os_str().to_owned(),
    ];
    match Command::new("pwsh").args(&args).output() {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Command::new("powershell.exe")
                .args(&args)
                .output()
                .map_err(|error| format!("run powershell.exe: {error}"))
        }
        Err(error) => Err(format!("run pwsh: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_script_uses_word_com() {
        assert!(SCRIPT.contains("Word.Application"));
        assert!(SCRIPT.contains("SaveAs2"));
        assert!(SCRIPT.contains("UTF8Encoding"));
    }

    #[test]
    fn batch_file_targets_each_image_with_text_only() {
        let path = std::env::temp_dir().join(format!(
            "deskctl-doc-batch-test-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let metadata = serde_json::json!({ "images": ["C:/tmp/a.png", "C:/tmp/b.jpg"] });
        write_batch_file(&path, &metadata).expect("write batch");
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .expect("read batch")
            .lines()
            .map(|line| serde_json::from_str(line).expect("batch line"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["tool"], "desktop_perceive");
        assert_eq!(lines[0]["arguments"]["text_only"], true);
        assert_eq!(lines[1]["arguments"]["image_path"], "C:/tmp/b.jpg");
        let _ = std::fs::remove_file(path);
    }
}
