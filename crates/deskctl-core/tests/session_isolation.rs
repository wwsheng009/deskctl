use std::time::Duration;

use deskctl_core::session::{CaptureState, ScreenRect, Session, TargetBinding};
use deskctl_core::Engine;
use deskctl_protocol::{ErrorCode, ToolName, ToolProfile};

fn capture(id: &str) -> CaptureState {
    CaptureState {
        id: id.to_string(),
        path: format!("C:/not-temp/{id}.png"),
        screen_rect: ScreenRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
        image_width: 10,
        image_height: 10,
        created_at: std::time::Instant::now(),
        elements: Default::default(),
        foreground_hwnd: None,
    }
}

#[test]
fn capture_and_target_do_not_cross_sessions() {
    let a = Session::new(ToolProfile::Standard, "a", 8, Duration::from_secs(120));
    let b = Session::new(ToolProfile::Standard, "b", 8, Duration::from_secs(120));
    a.store_capture(capture("cap-a")).unwrap();
    assert_eq!(a.capture("cap-a").unwrap().id, "cap-a");
    assert_eq!(
        b.capture("cap-a").expect_err("cross-session").code,
        ErrorCode::StaleCapture
    );
    a.store_target(TargetBinding {
        token: "target-a".into(),
        app_ref: "app:1:test.exe".into(),
        window_ref: Some("win:1".into()),
        hwnd: 1,
        title: "test".into(),
        created_at: std::time::Instant::now(),
    })
    .unwrap();
    assert!(a.target("target-a").is_ok());
    assert_eq!(
        b.target("target-a").expect_err("cross-session").code,
        ErrorCode::TargetUnavailable
    );
}

#[test]
fn closing_session_clears_tokens() {
    let session = Session::new(ToolProfile::Standard, "a", 8, Duration::from_secs(120));
    session.store_capture(capture("cap-a")).unwrap();
    session.close();
    assert!(session.is_closed());
    assert_eq!(
        session.capture("cap-a").expect_err("closed").code,
        ErrorCode::ServerRestarted
    );
}

#[test]
fn session_limit_returns_resource_busy() {
    let config = deskctl_core::Config {
        max_sessions: 1,
        ..Default::default()
    };
    let engine = Engine::new(config);
    engine
        .create_session(Some(ToolProfile::Standard), "a")
        .unwrap();
    assert_eq!(
        engine
            .create_session(Some(ToolProfile::Standard), "b")
            .expect_err("second session")
            .code,
        ErrorCode::ResourceBusy
    );
}

#[test]
fn readonly_profile_rejects_input() {
    let session = Session::new(ToolProfile::Readonly, "a", 8, Duration::from_secs(120));
    assert!(session.allows(ToolName::Screenshot));
    assert!(session.allows(ToolName::WindowsList));
    assert!(!session.allows(ToolName::Mouse));
    assert!(!session.allows(ToolName::Input));
    assert!(!session.allows(ToolName::ClipboardWrite));
}
