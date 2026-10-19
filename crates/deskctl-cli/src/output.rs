use serde::Serialize;
use std::path::Path;

/// Print a JSON result, or write it to a UTF-8 file when `output` is set.
///
/// Writing directly avoids the Windows console code-page ambiguity that can
/// turn UTF-8 daemon output into mojibake when a script captures stdout.
pub fn print_json_with_options<T: Serialize + ?Sized>(
    value: &T,
    compact: bool,
    output: Option<&Path>,
) -> Result<(), String> {
    let text = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
    .map_err(|error| error.to_string())?;

    match output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        format!("create output directory '{}': {error}", parent.display())
                    })?;
                }
            }
            std::fs::write(path, format!("{text}\n"))
                .map_err(|error| format!("write output '{}': {error}", path.display()))
        }
        None => {
            println!("{text}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::Value;

    #[test]
    fn writes_utf8_json_to_file() {
        let path = std::env::temp_dir().join(format!(
            "deskctl-output-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let value = json!({ "text": "后台作业 SM37", "ok": true });
        print_json_with_options(&value, true, Some(&path)).expect("write json");
        let bytes = std::fs::read(&path).expect("read json");
        let text = String::from_utf8(bytes).expect("output must be utf-8");
        let parsed: Value = serde_json::from_str(text.trim()).expect("valid json");
        assert_eq!(parsed["text"], "后台作业 SM37");
        assert_eq!(parsed["ok"], true);
        let _ = std::fs::remove_file(path);
    }
}
