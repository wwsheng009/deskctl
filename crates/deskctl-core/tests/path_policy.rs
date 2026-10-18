use std::time::Duration;

use deskctl_core::session::Session;
use deskctl_core::Config;

#[test]
fn dict_name_traversal_is_rejected() {
    let session = Session::new(
        deskctl_protocol::ToolProfile::Standard,
        "a",
        4,
        Duration::from_secs(120),
    );
    for dict in ["../keys", "a/b", r"a\b", "C:\\keys.dict"] {
        let args = serde_json::json!({
            "dict_name": dict,
            "words": "x",
            "region": { "x": 0, "y": 0, "width": 10, "height": 10 }
        });
        let result = deskctl_core::tools::execute_sync_for_test(
            &session,
            deskctl_protocol::ToolName::FindText,
            &args,
        );
        assert_eq!(
            result.unwrap_err().code,
            deskctl_protocol::ErrorCode::PolicyDenied,
            "{dict}"
        );
    }
}

#[test]
fn explicit_format_overrides_path_extension() {
    let config = Config::default();
    let directory = std::env::temp_dir().join("deskctl-path-policy");
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
    let jpg = deskctl_core::capture_policy::encode_and_save_with_format(
        &image,
        Some("shot.jpg"),
        &directory,
        &config.capture,
        None,
    )
    .unwrap();
    assert_eq!(jpg.path.extension().and_then(|v| v.to_str()), Some("jpg"));
    let png = deskctl_core::capture_policy::encode_and_save_with_format(
        &image,
        Some("shot.jpg"),
        &directory,
        &config.capture,
        Some("png"),
    )
    .unwrap();
    assert_eq!(png.path.extension().and_then(|v| v.to_str()), Some("png"));
    assert_eq!(png.format.as_str(), "png");
}
