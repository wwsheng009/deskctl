pub(crate) mod semantic;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use deskctl_protocol::{DispatchState, Effect, ErrorCode, ToolError, ToolName, Verification};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::{CancelFlag, Engine};
use crate::humanize::{self, HumanizeMode, Pcg32};
use crate::platform;
use crate::session::{
    CaptureState, ElementLocation, PressedKind, ScreenRect, Session, TargetBinding,
};
use crate::vision::locate;

#[derive(Debug)]
pub struct ToolExecution {
    pub data: Value,
    pub dispatch_state: DispatchState,
    pub effect: Option<Effect>,
    pub verification: Verification,
    pub requires_observation: bool,
}

impl ToolExecution {
    pub fn read(data: Value) -> Self {
        Self {
            data,
            dispatch_state: DispatchState::NotSent,
            effect: Some(Effect::Read),
            verification: Verification::NotApplicable,
            requires_observation: false,
        }
    }

    pub fn wrote(data: Value, state: DispatchState) -> Self {
        Self {
            data,
            dispatch_state: state,
            effect: Some(Effect::Unverifiable),
            verification: Verification::Unknown,
            requires_observation: true,
        }
    }

    pub fn verified(data: Value, state: DispatchState) -> Self {
        Self {
            data,
            dispatch_state: state,
            effect: Some(Effect::Unverifiable),
            verification: Verification::Satisfied,
            requires_observation: false,
        }
    }
}

pub async fn execute(
    engine: &std::sync::Arc<Engine>,
    session: &std::sync::Arc<Session>,
    tool: ToolName,
    args: &Value,
    cancel: &CancelFlag,
) -> Result<ToolExecution, ToolError> {
    // 脚本执行会回调引擎（`execute_leaf_with_session`），构成异步递归；
    // 在此处装箱以打断 future 类型推导环。
    if tool == ToolName::ScriptRun {
        return Box::pin(crate::script::run(engine, session, args, cancel)).await;
    }
    execute_leaf(engine, session, tool, args, cancel).await
}

/// 叶子工具调度：不包含 `desktop_script_run`（脚本步骤禁止嵌套脚本）。
/// 与 [`execute`] 分开是为了让 `execute_inner -> execute -> script::run ->
/// execute_leaf_with_session -> execute_leaf` 的 future 类型推导保持无环。
pub(crate) async fn execute_leaf(
    engine: &std::sync::Arc<Engine>,
    session: &std::sync::Arc<Session>,
    tool: ToolName,
    args: &Value,
    cancel: &CancelFlag,
) -> Result<ToolExecution, ToolError> {
    if cancel.load(Ordering::Acquire) {
        return Err(ToolError::new(
            ErrorCode::Cancelled,
            "request was cancelled",
        ));
    }
    match tool {
        ToolName::Screenshot => screenshot(engine, session, args),
        ToolName::ScreenSize => {
            let (width, height) = platform::screen_size()?;
            Ok(ToolExecution::read(
                json!({ "width": width, "height": height }),
            ))
        }
        ToolName::WindowsList => {
            let windows = platform::windows_list()?;
            Ok(ToolExecution::read(json!({
                "count": windows.len(),
                "windows": windows
            })))
        }
        ToolName::WindowInfo => {
            let hwnd = required_i64(args, "hwnd")?;
            Ok(ToolExecution::read(platform::window_info(hwnd)?))
        }
        ToolName::WindowActivate => window_activate(args),
        ToolName::WindowMove => window_move(args),
        ToolName::WindowResize => window_resize(args),
        ToolName::WindowScreenshot => window_screenshot(engine, session, args),
        ToolName::ImageCrop => image_crop(engine, session, args),
        ToolName::DocxRead => docx_read(engine, args),
        ToolName::DocxSave => docx_save(engine, session, args),
        ToolName::XlsxRead => xlsx_read(engine, args),
        ToolName::XlsxSave => xlsx_save(args),
        ToolName::PptxRead => pptx_read(engine, args),
        ToolName::PptxSave => pptx_save(engine, session, args),
        ToolName::Mouse => mouse(session, args, cancel),
        ToolName::MouseDrag => mouse_drag(session, args, cancel),
        ToolName::Input => input(session, args, cancel),
        ToolName::ClipboardWrite => clipboard_write(args),
        ToolName::ClipboardClean => clipboard_clean(),
        ToolName::FindImage => find_image(engine, args),
        ToolName::FindColor => find_color(args),
        ToolName::FindMultiColor => find_multi_color(args),
        ToolName::FindText => find_text(engine, args),
        ToolName::WaitFor => wait_for(args, cancel).await,
        ToolName::Perceive => perceive(engine, session, args),
        ToolName::ReadKv => read_kv(engine, session, args),
        ToolName::Vision => Err(ToolError::new(
            ErrorCode::ModelUnavailable,
            "desktop_vision requires an external vision provider; none is configured",
        )
        .retryable(false)),
        ToolName::TargetsList => targets_list(args),
        ToolName::TargetBind => target_bind(session, args),
        ToolName::RecorderStart => Ok(ToolExecution::read(crate::recorder::start(session, args)?)),
        ToolName::RecorderStop => Ok(ToolExecution::read(crate::recorder::stop(
            engine, session, args,
        )?)),
        // 脚本执行会回调 `Engine::execute_with_session`，构成异步递归；
        // 在此处装箱以打断 future 类型/Send 推导的循环。
        ToolName::ScriptRun => Err(ToolError::new(
            ErrorCode::Internal,
            "desktop_script_run cannot be nested inside a script",
        )),
        ToolName::FileHash => file_hash(engine, args),
        ToolName::CheckpointSave => crate::checkpoint::save(engine, session, args),
        ToolName::CheckpointVerify => crate::checkpoint::verify(engine, session, args),
        ToolName::SemanticObserve
        | ToolName::SemanticCandidate
        | ToolName::SemanticExecute
        | ToolName::SemanticAction
        | ToolName::VerifyState
        | ToolName::AgentStep => semantic::execute(engine, session, tool, args),
    }
}

fn screenshot(
    engine: &Engine,
    session: &Session,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    let capture = platform::capture_screen(optional_region(args))?;
    let policy = crate::capture_policy::CapturePolicy::from_args(args, &engine.config().capture)?;
    let encoded = platform::save_capture_with_format(
        &capture.image,
        args.get("path").and_then(Value::as_str),
        &engine.config().output_dir,
        &policy,
        args.get("format").and_then(Value::as_str),
    )?;
    let capture_id = store_capture(
        session,
        &capture,
        encoded.path.to_string_lossy().into_owned(),
    )?;
    Ok(ToolExecution::read(json!({
        "capture_id": capture_id,
        "path": encoded.path.to_string_lossy(),
        "format": encoded.format.as_str(),
        "width": encoded.width,
        "height": encoded.height,
        "source_width": capture.image.width(),
        "source_height": capture.image.height(),
        "bytes": encoded.bytes,
        "resized": encoded.resized,
        "quality": encoded.quality,
        "screen_region": capture.screen_rect,
        "foreground_hwnd": capture.foreground_hwnd
    })))
}

fn window_activate(args: &Value) -> Result<ToolExecution, ToolError> {
    let hwnd = required_i64(args, "hwnd")?;
    let report = platform::activate_window_report(hwnd)?;
    if !report.active {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            format!(
                "window could not be brought to the foreground after {} attempts",
                report.attempts
            ),
        )
        .details(json!({
            "attempts": report.attempts,
            "method": report.method
        }))
        .retryable(true));
    }
    Ok(ToolExecution::verified(
        json!({
            "hwnd": hwnd,
            "foreground": true,
            "attempts": report.attempts,
            "method": report.method
        }),
        DispatchState::Sent,
    ))
}

fn window_move(args: &Value) -> Result<ToolExecution, ToolError> {
    let hwnd = required_i64(args, "hwnd")?;
    let x = required_i32(args, "x")?;
    let y = required_i32(args, "y")?;
    let before = platform::window_info(hwnd)?;
    platform::move_window(hwnd, x, y)?;
    let after = platform::window_info(hwnd)?;
    let verified = after["x"] == x && after["y"] == y;
    if !verified {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            "window move was accepted but the new bounds were not observed",
        )
        .details(json!({ "dispatch_state": DispatchState::Unknown }))
        .retryable(false));
    }
    Ok(ToolExecution::verified(
        json!({ "before": before, "after": after }),
        DispatchState::Sent,
    ))
}

fn window_resize(args: &Value) -> Result<ToolExecution, ToolError> {
    let hwnd = required_i64(args, "hwnd")?;
    let width = required_u32(args, "width")?;
    let height = required_u32(args, "height")?;
    let before = platform::window_info(hwnd)?;
    platform::resize_window(hwnd, width, height)?;
    let after = platform::window_info(hwnd)?;
    let verified = after["width"].as_u64() == Some(u64::from(width))
        && after["height"].as_u64() == Some(u64::from(height));
    if !verified {
        return Err(ToolError::new(
            ErrorCode::DispatchUnknown,
            "window resize was accepted but the new bounds were not observed",
        )
        .details(json!({ "dispatch_state": DispatchState::Unknown }))
        .retryable(false));
    }
    Ok(ToolExecution::verified(
        json!({ "before": before, "after": after }),
        DispatchState::Sent,
    ))
}

fn window_screenshot(
    engine: &Engine,
    session: &Session,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    let hwnd = if let Some(hwnd) = args.get("hwnd").and_then(Value::as_i64) {
        hwnd
    } else if let Some(title) = args.get("title").and_then(Value::as_str) {
        platform::find_window_by_title(title)?.ok_or_else(|| {
            ToolError::new(
                ErrorCode::TargetUnavailable,
                format!("no unique window matches title {title:?}"),
            )
            .retryable(true)
        })?
    } else {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "window_screenshot requires hwnd or title",
        ));
    };
    let mut capture = platform::capture_window(hwnd)?;
    let cropped = if let Some(region) = optional_region(args) {
        if region.width == 0 || region.height == 0 {
            return Err(invalid("region width and height must be positive"));
        }
        let image_width = i64::from(capture.image.width());
        let image_height = i64::from(capture.image.height());
        if region.x < 0
            || region.y < 0
            || i64::from(region.x) + i64::from(region.width) > image_width
            || i64::from(region.y) + i64::from(region.height) > image_height
        {
            return Err(invalid(format!(
                "region is outside the captured window ({}x{})",
                image_width, image_height
            )));
        }
        capture.image = image::imageops::crop_imm(
            &capture.image,
            region.x as u32,
            region.y as u32,
            region.width,
            region.height,
        )
        .to_image();
        capture.screen_rect.x += region.x;
        capture.screen_rect.y += region.y;
        capture.screen_rect.width = region.width;
        capture.screen_rect.height = region.height;
        true
    } else {
        false
    };
    let policy = crate::capture_policy::CapturePolicy::from_args(args, &engine.config().capture)?;
    let encoded = platform::save_capture_with_format(
        &capture.image,
        args.get("path").and_then(Value::as_str),
        &engine.config().output_dir,
        &policy,
        args.get("format").and_then(Value::as_str),
    )?;
    let capture_id = store_capture(
        session,
        &capture,
        encoded.path.to_string_lossy().into_owned(),
    )?;
    Ok(ToolExecution::read(json!({
        "capture_id": capture_id,
        "path": encoded.path.to_string_lossy(),
        "format": encoded.format.as_str(),
        "hwnd": hwnd,
        "width": encoded.width,
        "height": encoded.height,
        "source_width": capture.image.width(),
        "source_height": capture.image.height(),
        "bytes": encoded.bytes,
        "resized": encoded.resized,
        "quality": encoded.quality,
        "cropped": cropped,
        "screen_region": capture.screen_rect
    })))
}

fn mouse(session: &Session, args: &Value, cancel: &CancelFlag) -> Result<ToolExecution, ToolError> {
    let action = required_str(args, "action")?;
    if action == "position" {
        let (x, y) = platform::mouse_position()?;
        return Ok(ToolExecution::read(json!({ "x": x, "y": y })));
    }
    if !matches!(
        action,
        "click" | "double_click" | "hover" | "move" | "move_relative" | "down" | "up" | "scroll"
    ) {
        return Err(invalid(
            "unknown mouse action; expected click, double_click, hover, move, move_relative, down, up, scroll or position",
        ));
    }
    if let Some(hwnd) = optional_i64(args, "hwnd")? {
        let _ = ensure_foreground(hwnd)?;
    }
    let humanize = HumanizeMode::parse(args.get("humanize"))?;
    let speed = humanize::validate_speed(args.get("speed"))?;
    let hold_arg = humanize::validate_hold_ms(args.get("hold_ms"))?;
    let jitter_px = humanize::validate_jitter_px(args.get("jitter_px"))?;
    let mut seed = humanize::validate_seed(args.get("seed"))?;
    let has_capture = args.get("capture_id").is_some();
    let has_element = args.get("element_id").is_some();
    let has_coordinates = args.get("x").is_some() || args.get("y").is_some();
    if has_capture != has_element {
        return Err(invalid(
            "capture_id and element_id must be provided together",
        ));
    }
    if has_capture && has_coordinates {
        return Err(invalid(
            "capture_id + element_id cannot be mixed with raw x/y coordinates",
        ));
    }
    if action == "move_relative" && (has_capture || has_coordinates) {
        return Err(invalid("move_relative accepts dx/dy only"));
    }
    if action == "scroll" {
        let direction = args
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("down");
        let amount = args.get("amount").and_then(Value::as_u64).unwrap_or(3);
        platform::mouse_scroll(
            direction,
            u32::try_from(amount).map_err(|_| invalid("amount is too large"))?,
        )?;
        let (x, y) = platform::mouse_position()?;
        return Ok(ToolExecution::wrote(
            json!({
                "action": action,
                "x": x,
                "y": y,
                "dispatch_state": "sent"
            }),
            DispatchState::Sent,
        ));
    }
    // jitter 仅作用于"落点"类动作：点击族与 down；hover 保持既有语义。
    let jitter_enabled =
        jitter_px > 0 && matches!(action, "click" | "double_click" | "down" | "move");
    let target: Option<(i32, i32)> = if has_capture {
        let capture_id = required_str(args, "capture_id")?;
        let element_id = required_u32(args, "element_id")?;
        let element = resolve_element_location(session, capture_id, element_id)?;
        let center = element.screen_center.ok_or_else(|| {
            ToolError::new(
                ErrorCode::StaleCapture,
                "element has no registered screen center",
            )
            .retryable(true)
        })?;
        if jitter_enabled {
            let mut rng = humanize::rng_with_seed(&mut seed);
            Some(jitter_in_rect(&mut rng, &element.rect, center))
        } else {
            Some(center)
        }
    } else if has_coordinates {
        let x = required_i32(args, "x")?;
        let y = required_i32(args, "y")?;
        if jitter_enabled {
            let mut rng = humanize::rng_with_seed(&mut seed);
            Some(jitter_around(&mut rng, x, y, jitter_px))
        } else {
            Some((x, y))
        }
    } else {
        None
    };
    let speed = speed.unwrap_or(humanize::SPEED_DEFAULT);
    let cancellable = || cancel.load(Ordering::Acquire);
    match action {
        "click" | "double_click" => {
            let point = target.ok_or_else(|| {
                invalid("click requires x/y coordinates or capture_id + element_id")
            })?;
            let button = mouse_button_arg(args)?;
            let clicks = if action == "double_click" {
                2
            } else {
                args.get("clicks").and_then(Value::as_u64).unwrap_or(1)
            };
            if !(1..=2).contains(&clicks) {
                return Err(invalid("clicks must be 1 or 2"));
            }
            if !args
                .get("allow_occluded")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if let Some(expected) = expected_click_target(session, args)? {
                    if let Some(blocker) = occlusion_blocker(point.0, point.1, expected)? {
                        return Err(ToolError::new(
                            ErrorCode::TargetUnavailable,
                            format!(
                                "click target is occluded by another window (pid {}); pass allow_occluded=true to force",
                                blocker["process_id"]
                            ),
                        )
                        .details(json!({ "occluded_by": blocker }))
                        .retryable(true));
                    }
                }
            }
            if !humanize.enabled() && hold_arg.is_none() {
                platform::mouse_click(point.0, point.1, button, clicks as u8)?;
                return Ok(ToolExecution::wrote(
                    json!({
                        "action": action,
                        "x": point.0,
                        "y": point.1,
                        "button": button,
                        "clicks": clicks,
                        "dispatch_state": "sent"
                    }),
                    DispatchState::Sent,
                ));
            }
            // 冲突清理：同一按钮仍有未配对的按下时先释放，避免状态错乱。
            if session.clear_pressed(PressedKind::Button, button) {
                let _ = platform::mouse_button(button, false);
            }
            let mut rng = humanize::rng_with_seed(&mut seed);
            let mut path_points = None;
            if humanize.enabled() {
                let current = platform::mouse_position()?;
                let plan = humanize::plan_move(current, point, humanize, speed, &mut rng);
                humanize::ensure_total_cap(plan.total_ms)?;
                let sent = platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable)?;
                path_points = Some(sent);
                if sent < plan.points.len() {
                    return Ok(cancelled_mouse(action, point, humanize, seed, path_points));
                }
                let pause = humanize::sample_move_pause_ms(&mut rng);
                if pause > 0 && !humanize::sleep_cancellable(pause, &cancellable) {
                    return Ok(cancelled_mouse(action, point, humanize, seed, path_points));
                }
            } else {
                platform::mouse_move(point.0, point.1)?;
            }
            let hold =
                hold_arg.unwrap_or_else(|| humanize::sample_mouse_hold_ms(humanize, &mut rng));
            let gap =
                humanize::sample_click_gap_ms(humanize, &mut rng, platform::double_click_time_ms());
            let mut dispatched: u64 = 0;
            let mut cancelled = false;
            for index in 0..clicks {
                if cancel.load(Ordering::Acquire) {
                    cancelled = true;
                    break;
                }
                platform::mouse_button(button, true)?;
                if !humanize::sleep_cancellable(hold, &cancellable) {
                    let _ = platform::mouse_button(button, false);
                    cancelled = true;
                    break;
                }
                platform::mouse_button(button, false)?;
                dispatched = index + 1;
                if clicks > 1
                    && index + 1 < clicks
                    && !humanize::sleep_cancellable(gap, &cancellable)
                {
                    cancelled = true;
                    break;
                }
            }
            Ok(ToolExecution::wrote(
                json!({
                    "action": action,
                    "x": point.0,
                    "y": point.1,
                    "button": button,
                    "clicks": clicks,
                    "dispatched": dispatched,
                    "hold_ms": hold,
                    "click_gap_ms": gap,
                    "humanize": humanize.as_str(),
                    "seed": seed,
                    "path_points": path_points,
                    "cancelled": cancelled,
                    "dispatch_state": if cancelled { "partial" } else { "sent" }
                }),
                if cancelled {
                    DispatchState::Partial
                } else {
                    DispatchState::Sent
                },
            ))
        }
        "hover" | "move" => {
            let point = target.ok_or_else(|| {
                invalid("move/hover requires x/y coordinates or capture_id + element_id")
            })?;
            let mut path_points = None;
            if humanize.enabled() {
                let current = platform::mouse_position()?;
                let mut rng = humanize::rng_with_seed(&mut seed);
                let plan = humanize::plan_move(current, point, humanize, speed, &mut rng);
                humanize::ensure_total_cap(plan.total_ms)?;
                let sent = platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable)?;
                path_points = Some(sent);
                if sent < plan.points.len() {
                    return Ok(cancelled_mouse(action, point, humanize, seed, path_points));
                }
            } else {
                platform::mouse_move(point.0, point.1)?;
            }
            let actual = platform::mouse_position().unwrap_or(point);
            Ok(ToolExecution::wrote(
                json!({
                    "action": action,
                    "x": actual.0,
                    "y": actual.1,
                    "humanize": humanize.as_str(),
                    "seed": seed,
                    "path_points": path_points,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        }
        "move_relative" => {
            let dx = required_i32(args, "dx")?;
            let dy = required_i32(args, "dy")?;
            let current = platform::mouse_position()?;
            let destination = (current.0.saturating_add(dx), current.1.saturating_add(dy));
            let mut path_points = None;
            if humanize.enabled() {
                let mut rng = humanize::rng_with_seed(&mut seed);
                let plan = humanize::plan_move(current, destination, humanize, speed, &mut rng);
                humanize::ensure_total_cap(plan.total_ms)?;
                let sent = platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable)?;
                path_points = Some(sent);
                if sent < plan.points.len() {
                    return Ok(cancelled_mouse(
                        action,
                        destination,
                        humanize,
                        seed,
                        path_points,
                    ));
                }
            } else {
                platform::mouse_move(destination.0, destination.1)?;
            }
            let actual = platform::mouse_position().unwrap_or(destination);
            Ok(ToolExecution::wrote(
                json!({
                    "action": action,
                    "x": actual.0,
                    "y": actual.1,
                    "dx": dx,
                    "dy": dy,
                    "humanize": humanize.as_str(),
                    "seed": seed,
                    "path_points": path_points,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        }
        "down" => {
            let button = mouse_button_arg(args)?;
            let cleaned = session.clear_pressed(PressedKind::Button, button);
            if cleaned {
                let _ = platform::mouse_button(button, false);
            }
            if let Some(point) = target {
                if !args
                    .get("allow_occluded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    if let Some(expected) = expected_click_target(session, args)? {
                        if let Some(blocker) = occlusion_blocker(point.0, point.1, expected)? {
                            return Err(ToolError::new(
                                ErrorCode::TargetUnavailable,
                                format!(
                                    "press target is occluded by another window (pid {}); pass allow_occluded=true to force",
                                    blocker["process_id"]
                                ),
                            )
                            .details(json!({ "occluded_by": blocker }))
                            .retryable(true));
                        }
                    }
                }
                if humanize.enabled() {
                    let current = platform::mouse_position()?;
                    let mut rng = humanize::rng_with_seed(&mut seed);
                    let plan = humanize::plan_move(current, point, humanize, speed, &mut rng);
                    humanize::ensure_total_cap(plan.total_ms)?;
                    let sent =
                        platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable)?;
                    if sent < plan.points.len() {
                        return Ok(cancelled_mouse(action, point, humanize, seed, Some(sent)));
                    }
                } else {
                    platform::mouse_move(point.0, point.1)?;
                }
            }
            platform::mouse_button(button, true)?;
            session.mark_pressed(PressedKind::Button, button);
            let actual = platform::mouse_position().unwrap_or((0, 0));
            Ok(ToolExecution::wrote(
                json!({
                    "action": action,
                    "button": button,
                    "x": actual.0,
                    "y": actual.1,
                    "pressed": true,
                    "cleaned_pressed": cleaned,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        }
        "up" => {
            let button = mouse_button_arg(args)?;
            if let Some(point) = target {
                if humanize.enabled() {
                    let current = platform::mouse_position()?;
                    let mut rng = humanize::rng_with_seed(&mut seed);
                    let plan = humanize::plan_move(current, point, humanize, speed, &mut rng);
                    humanize::ensure_total_cap(plan.total_ms)?;
                    let sent =
                        platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable)?;
                    if sent < plan.points.len() {
                        // 取消也必须释放按钮，避免粘键。
                        let was_tracked = session.clear_pressed(PressedKind::Button, button);
                        platform::mouse_button(button, false)?;
                        return Ok(ToolExecution::wrote(
                            json!({
                                "action": action,
                                "button": button,
                                "released": true,
                                "was_tracked": was_tracked,
                                "path_points": sent,
                                "cancelled": true,
                                "dispatch_state": "partial"
                            }),
                            DispatchState::Partial,
                        ));
                    }
                } else {
                    platform::mouse_move(point.0, point.1)?;
                }
            }
            let was_tracked = session.clear_pressed(PressedKind::Button, button);
            platform::mouse_button(button, false)?;
            let actual = platform::mouse_position().unwrap_or((0, 0));
            Ok(ToolExecution::wrote(
                json!({
                    "action": action,
                    "button": button,
                    "released": true,
                    "was_tracked": was_tracked,
                    "x": actual.0,
                    "y": actual.1,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        }
        _ => unreachable!(),
    }
}

/// 鼠标按钮参数解析（默认 left）。
fn mouse_button_arg(args: &Value) -> Result<&str, ToolError> {
    let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
    if !matches!(button, "left" | "right" | "middle") {
        return Err(invalid("button must be left, right or middle"));
    }
    Ok(button)
}

/// 长序列被取消时的统一响应：如实报告已发送量。
fn cancelled_mouse(
    action: &str,
    point: (i32, i32),
    humanize: HumanizeMode,
    seed: Option<u64>,
    path_points: Option<usize>,
) -> ToolExecution {
    ToolExecution::wrote(
        json!({
            "action": action,
            "x": point.0,
            "y": point.1,
            "humanize": humanize.as_str(),
            "seed": seed,
            "path_points": path_points,
            "cancelled": true,
            "dispatch_state": "partial"
        }),
        DispatchState::Partial,
    )
}

/// 点击期望落在哪个窗口：显式 hwnd 优先，其次本次捕获的前台窗口。
fn expected_click_target(session: &Session, args: &Value) -> Result<Option<i64>, ToolError> {
    if let Some(hwnd) = optional_i64(args, "hwnd")? {
        return Ok(Some(hwnd));
    }
    if let Some(capture_id) = args.get("capture_id").and_then(Value::as_str) {
        if let Ok(capture) = session.capture(capture_id) {
            if let Some(foreground) = capture.foreground_hwnd {
                return Ok(Some(foreground));
            }
        }
    }
    Ok(None)
}

/// 点击点被其它窗口遮挡时返回遮挡者信息；同进程子窗口与任务栏（控制窗口切换）
/// 视为合法目标，避免误报。
fn occlusion_blocker(x: i32, y: i32, expected_hwnd: i64) -> Result<Option<Value>, ToolError> {
    let Some(top) = platform::window_at_point(x, y)? else {
        return Ok(None);
    };
    let top_hwnd = top["hwnd"].as_i64().unwrap_or_default();
    let top_pid = top["process_id"].as_u64();
    let class_name = top["class_name"].as_str().unwrap_or_default();
    if matches!(class_name, "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return Ok(None);
    }
    let expected = platform::window_info(expected_hwnd)?;
    let expected_pid = expected["process_id"].as_u64();
    if top_hwnd == expected_hwnd || (top_pid.is_some() && top_pid == expected_pid) {
        return Ok(None);
    }
    Ok(Some(json!({
        "hwnd": top_hwnd,
        "process_id": top_pid,
        "process_name": top["process_name"],
        "title": top["title"]
    })))
}

fn mouse_drag(
    session: &Session,
    args: &Value,
    cancel: &CancelFlag,
) -> Result<ToolExecution, ToolError> {
    let start = resolve_drag_point(
        session,
        args,
        "start_capture_id",
        "start_element_id",
        "start_x",
        "start_y",
    )?;
    let end = resolve_drag_point(
        session,
        args,
        "end_capture_id",
        "end_element_id",
        "end_x",
        "end_y",
    )?;
    let duration = args
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(250)
        .clamp(20, 5000);
    let humanize = HumanizeMode::parse(args.get("humanize"))?;
    let speed_arg = humanize::validate_speed(args.get("speed"))?;
    let path_arg = args.get("path").and_then(Value::as_str);
    if let Some(path) = path_arg {
        if !matches!(path, "line" | "curve") {
            return Err(invalid("path must be line or curve"));
        }
    }
    let mut seed = humanize::validate_seed(args.get("seed"))?;
    let enhanced = humanize.enabled() || speed_arg.is_some() || path_arg.is_some();
    if !enhanced {
        platform::drag(start, end, duration)?;
        return Ok(ToolExecution::wrote(
            json!({
                "start": { "x": start.0, "y": start.1 },
                "end": { "x": end.0, "y": end.1 },
                "dispatch_state": "sent"
            }),
            DispatchState::Sent,
        ));
    }
    let mut rng = humanize::rng_with_seed(&mut seed);
    let distance = (((end.0 - start.0) as f64).powi(2) + ((end.1 - start.1) as f64).powi(2)).sqrt();
    // 默认速度由 duration_ms 反推，保持与旧参数一致的节奏。
    let speed = speed_arg.unwrap_or_else(|| {
        if duration > 0 && distance >= 1.0 {
            (distance / (duration as f64 / 1000.0)).clamp(humanize::SPEED_MIN, humanize::SPEED_MAX)
        } else {
            humanize::SPEED_DEFAULT
        }
    });
    let curved = match path_arg {
        Some("curve") => true,
        Some("line") => false,
        _ => humanize.enabled(),
    };
    let plan = humanize::plan_path(start, end, curved, humanize, speed, &mut rng);
    humanize::ensure_total_cap(plan.total_ms)?;
    let cancellable = || cancel.load(Ordering::Acquire);
    platform::mouse_move(start.0, start.1)?;
    // 冲突清理：左键仍处于按下时先释放，避免拖拽状态错乱。
    if session.clear_pressed(PressedKind::Button, "left") {
        let _ = platform::mouse_button("left", false);
    }
    platform::mouse_button("left", true)?;
    let sent = match platform::mouse_move_path(&plan.points, &plan.delays_ms, &cancellable) {
        Ok(sent) => sent,
        Err(error) => {
            let _ = platform::mouse_button("left", false);
            return Err(error);
        }
    };
    platform::mouse_button("left", false)?;
    let cancelled = sent < plan.points.len();
    Ok(ToolExecution::wrote(
        json!({
            "start": { "x": start.0, "y": start.1 },
            "end": { "x": end.0, "y": end.1 },
            "humanize": humanize.as_str(),
            "seed": seed,
            "path": if curved { "curve" } else { "line" },
            "speed": speed,
            "path_points": sent,
            "total_points": plan.points.len(),
            "cancelled": cancelled,
            "dispatch_state": if cancelled { "partial" } else { "sent" }
        }),
        if cancelled {
            DispatchState::Partial
        } else {
            DispatchState::Sent
        },
    ))
}

fn input(session: &Session, args: &Value, cancel: &CancelFlag) -> Result<ToolExecution, ToolError> {
    let mode = required_str(args, "mode")?;
    if !matches!(mode, "type" | "hotkey" | "key" | "key_down" | "key_up") {
        return Err(invalid(
            "mode must be type, hotkey, key, key_down or key_up",
        ));
    }
    let hwnd = match resolve_input_target(session, args) {
        Ok(hwnd) => hwnd,
        Err(error) => match args.get("launch_ref").and_then(Value::as_str) {
            Some(launch_ref) => match relaunch_and_retry_input(session, args, launch_ref) {
                Ok(hwnd) => hwnd,
                Err(_) => return Err(error),
            },
            None => return Err(error),
        },
    };
    let mut target_hwnd: Option<i64> = None;
    if let Some(hwnd) = hwnd {
        target_hwnd = Some(ensure_foreground(hwnd)?);
    }
    let humanize = HumanizeMode::parse(args.get("humanize"))?;
    let mut seed = humanize::validate_seed(args.get("seed"))?;
    match mode {
        "type" => input_type(args, humanize, &mut seed, cancel, target_hwnd),
        "hotkey" => input_hotkey(args, humanize, &mut seed, cancel, target_hwnd),
        _ => input_key(
            session,
            mode,
            args,
            humanize,
            &mut seed,
            cancel,
            target_hwnd,
        ),
    }
}

/// `send` 参数：type 默认 enter，其余模式默认 none。
fn send_arg<'a>(args: &'a Value, default: &'a str) -> Result<&'a str, ToolError> {
    let send = args.get("send").and_then(Value::as_str).unwrap_or(default);
    if !matches!(send, "enter" | "ctrl+enter" | "tab" | "none") {
        return Err(invalid("send must be enter, ctrl+enter, tab or none"));
    }
    Ok(send)
}

/// `delay_ms`：固定值或 `[min, max]` 范围（毫秒）。
fn parse_delay_ms(value: Option<&Value>) -> Result<Option<(u64, u64)>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let (min, max) = if let Some(fixed) = value.as_u64() {
        (fixed, fixed)
    } else if let Some(range) = value.as_array() {
        if range.len() != 2 {
            return Err(invalid("delay_ms range must be [min, max]"));
        }
        let min = range[0]
            .as_u64()
            .ok_or_else(|| invalid("delay_ms values must be unsigned integers"))?;
        let max = range[1]
            .as_u64()
            .ok_or_else(|| invalid("delay_ms values must be unsigned integers"))?;
        (min, max)
    } else {
        return Err(invalid("delay_ms must be a number or [min, max]"));
    };
    if min > max {
        return Err(invalid("delay_ms min must not exceed max"));
    }
    if max > humanize::DELAY_MS_MAX {
        return Err(invalid(format!(
            "delay_ms must not exceed {}",
            humanize::DELAY_MS_MAX
        )));
    }
    Ok(Some((min, max)))
}

/// 执行尾随按键；失败时文本已经输入，必须报 Partial。
fn send_trailing_key(send: &str) -> Result<(), ToolError> {
    if send == "none" {
        return Ok(());
    }
    let result = match send {
        "enter" => platform::hotkey(&["enter"]),
        "ctrl+enter" => platform::hotkey(&["ctrl", "enter"]),
        "tab" => platform::hotkey(&["tab"]),
        _ => Ok(()),
    };
    result.map_err(|error| {
        ToolError::new(
            ErrorCode::DispatchUnknown,
            format!("input was dispatched but the trailing key failed: {error}"),
        )
        .details(json!({ "dispatch_state": DispatchState::Partial }))
        .retryable(false)
    })
}

fn input_type(
    args: &Value,
    humanize: HumanizeMode,
    seed: &mut Option<u64>,
    cancel: &CancelFlag,
    target_hwnd: Option<i64>,
) -> Result<ToolExecution, ToolError> {
    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
    if text.is_empty() {
        return Err(invalid("text is required for mode=type"));
    }
    let send = send_arg(args, "enter")?;
    let explicit = parse_delay_ms(args.get("delay_ms"))?;
    if !humanize.enabled() && explicit.is_none() {
        platform::type_text(text).map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
        send_trailing_key(send)?;
        return Ok(ToolExecution::wrote(
            json!({ "mode": "type", "target_hwnd": target_hwnd, "dispatch_state": "sent" }),
            DispatchState::Sent,
        ));
    }
    let mut rng = humanize::rng_with_seed(seed);
    let plan = humanize::plan_typing(text, humanize, explicit, &mut rng);
    humanize::ensure_total_cap(plan.total_ms)?;
    let cancellable = || cancel.load(Ordering::Acquire);
    let mut dispatched = 0usize;
    for (index, ch) in plan.chars.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let mut buffer = [0u8; 4];
        let single = ch.encode_utf8(&mut buffer);
        platform::type_text(single)
            .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
        dispatched += 1;
        if let Some(delay) = plan.delays_ms.get(index) {
            if *delay > 0 && !humanize::sleep_cancellable(*delay, &cancellable) {
                break;
            }
        }
    }
    let total_chars = plan.chars.len();
    if dispatched < total_chars {
        return Ok(ToolExecution::wrote(
            json!({
                "mode": "type",
                "target_hwnd": target_hwnd,
                "humanize": humanize.as_str(),
                "seed": seed,
                "dispatched_chars": dispatched,
                "total_chars": total_chars,
                "cancelled": true,
                "dispatch_state": "partial"
            }),
            DispatchState::Partial,
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(ToolExecution::wrote(
            json!({
                "mode": "type",
                "target_hwnd": target_hwnd,
                "humanize": humanize.as_str(),
                "seed": seed,
                "dispatched_chars": dispatched,
                "total_chars": total_chars,
                "cancelled": true,
                "dispatch_state": "partial"
            }),
            DispatchState::Partial,
        ));
    }
    send_trailing_key(send)?;
    Ok(ToolExecution::wrote(
        json!({
            "mode": "type",
            "target_hwnd": target_hwnd,
            "humanize": humanize.as_str(),
            "seed": seed,
            "dispatched_chars": dispatched,
            "total_chars": total_chars,
            "delay_total_ms": plan.total_ms,
            "dispatch_state": "sent"
        }),
        DispatchState::Sent,
    ))
}

fn input_hotkey(
    args: &Value,
    humanize: HumanizeMode,
    seed: &mut Option<u64>,
    cancel: &CancelFlag,
    target_hwnd: Option<i64>,
) -> Result<ToolExecution, ToolError> {
    let keys = args
        .get("keys")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| invalid("keys must contain strings"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let keys = keys.ok_or_else(|| invalid("keys are required for mode=hotkey"))?;
    if keys.is_empty() {
        return Err(invalid("keys must not be empty"));
    }
    for key in &keys {
        if !platform::is_valid_key(key) {
            return Err(invalid(format!("unknown key: {key}")));
        }
    }
    let hold_arg = humanize::validate_hold_ms(args.get("hold_ms"))?;
    let repeat = humanize::validate_repeat(args.get("repeat"))?.unwrap_or(1);
    let interval_arg = humanize::validate_interval_ms(args.get("interval_ms"))?;
    if hold_arg.is_none() && !humanize.enabled() && repeat == 1 {
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        platform::hotkey(&refs).map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
        return Ok(ToolExecution::wrote(
            json!({ "mode": "hotkey", "target_hwnd": target_hwnd, "dispatch_state": "sent" }),
            DispatchState::Sent,
        ));
    }
    let mut rng = humanize::rng_with_seed(seed);
    let hold = hold_arg.unwrap_or_else(|| humanize::sample_key_hold_ms(humanize, &mut rng));
    let interval = interval_arg.unwrap_or(120);
    let cancellable = || cancel.load(Ordering::Acquire);
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let mut dispatched: u64 = 0;
    for index in 0..repeat {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let completed = platform::hotkey_hold(&refs, hold, &cancellable)
            .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
        dispatched += 1;
        if !completed {
            break;
        }
        if index + 1 < repeat && !humanize::sleep_cancellable(interval, &cancellable) {
            break;
        }
    }
    let cancelled = dispatched < repeat;
    Ok(ToolExecution::wrote(
        json!({
            "mode": "hotkey",
            "target_hwnd": target_hwnd,
            "humanize": humanize.as_str(),
            "seed": seed,
            "hold_ms": hold,
            "repeat": repeat,
            "interval_ms": interval,
            "dispatched": dispatched,
            "cancelled": cancelled,
            "dispatch_state": if cancelled { "partial" } else { "sent" }
        }),
        if cancelled {
            DispatchState::Partial
        } else {
            DispatchState::Sent
        },
    ))
}

/// key 的稳定标识：优先 scancode（含扩展位），否则 vk。
fn key_spec_id(spec: &platform::KeySpec) -> String {
    if let Some(scan) = spec.scan {
        let raw = if spec.extended {
            0xE000u32 | u32::from(scan)
        } else {
            u32::from(scan)
        };
        format!("scan:0x{raw:X}")
    } else {
        format!("vk:{}", spec.vk.unwrap_or_default())
    }
}

fn input_key(
    session: &Session,
    mode: &str,
    args: &Value,
    humanize: HumanizeMode,
    seed: &mut Option<u64>,
    cancel: &CancelFlag,
    target_hwnd: Option<i64>,
) -> Result<ToolExecution, ToolError> {
    let spec_text = required_str(args, "key")?;
    let spec = platform::parse_key_spec(spec_text)?;
    let force_extended = args
        .get("extended")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hold_arg = humanize::validate_hold_ms(args.get("hold_ms"))?;
    let repeat_arg = humanize::validate_repeat(args.get("repeat"))?;
    let interval_arg = humanize::validate_interval_ms(args.get("interval_ms"))?;
    let send = send_arg(args, "none")?;
    let cancellable = || cancel.load(Ordering::Acquire);
    if mode == "key_down" || mode == "key_up" {
        if hold_arg.is_some() || repeat_arg.is_some() || interval_arg.is_some() {
            return Err(invalid(
                "hold_ms, repeat and interval_ms apply to mode=key only",
            ));
        }
        let id = key_spec_id(&spec);
        if mode == "key_down" {
            let cleaned = session.clear_pressed(PressedKind::Key, &id);
            if cleaned {
                let _ = platform::send_key(&spec, true, force_extended);
            }
            platform::send_key(&spec, false, force_extended)
                .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
            session.mark_pressed(PressedKind::Key, &id);
            Ok(ToolExecution::wrote(
                json!({
                    "mode": mode,
                    "target_hwnd": target_hwnd,
                    "key": spec_text,
                    "key_mode": spec.source,
                    "pressed": true,
                    "cleaned_pressed": cleaned,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        } else {
            let was_tracked = session.clear_pressed(PressedKind::Key, &id);
            platform::send_key(&spec, true, force_extended)
                .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
            Ok(ToolExecution::wrote(
                json!({
                    "mode": mode,
                    "target_hwnd": target_hwnd,
                    "key": spec_text,
                    "key_mode": spec.source,
                    "released": true,
                    "was_tracked": was_tracked,
                    "dispatch_state": "sent"
                }),
                DispatchState::Sent,
            ))
        }
    } else {
        let repeat = repeat_arg.unwrap_or(1);
        let mut rng = humanize::rng_with_seed(seed);
        let hold = hold_arg.unwrap_or_else(|| humanize::sample_key_hold_ms(humanize, &mut rng));
        let interval = interval_arg.unwrap_or(120);
        let mut dispatched: u64 = 0;
        for index in 0..repeat {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            platform::send_key(&spec, false, force_extended)
                .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
            if !humanize::sleep_cancellable(hold, &cancellable) {
                let _ = platform::send_key(&spec, true, force_extended);
                dispatched += 1;
                break;
            }
            platform::send_key(&spec, true, force_extended)
                .map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
            dispatched += 1;
            if index + 1 < repeat && !humanize::sleep_cancellable(interval, &cancellable) {
                break;
            }
        }
        let cancelled = dispatched < repeat;
        if !cancelled {
            send_trailing_key(send)?;
        }
        Ok(ToolExecution::wrote(
            json!({
                "mode": mode,
                "target_hwnd": target_hwnd,
                "key": spec_text,
                "key_mode": spec.source,
                "humanize": humanize.as_str(),
                "seed": seed,
                "hold_ms": hold,
                "repeat": repeat,
                "interval_ms": interval,
                "dispatched": dispatched,
                "cancelled": cancelled,
                "dispatch_state": if cancelled { "partial" } else { "sent" }
            }),
            if cancelled {
                DispatchState::Partial
            } else {
                DispatchState::Sent
            },
        ))
    }
}

/// 目标窗口缺失时，用 catalog 引用重启应用并重试定位；只接受
/// desktop_targets_list 返回的 app_ref，绝不接受任意可执行路径。
fn relaunch_and_retry_input(
    session: &Session,
    args: &Value,
    launch_ref: &str,
) -> Result<Option<i64>, ToolError> {
    let entry = crate::apps::find(launch_ref).ok_or_else(|| {
        ToolError::new(
            ErrorCode::InvalidParams,
            "launch_ref must be an app_ref returned by desktop_targets_list",
        )
        .retryable(false)
    })?;
    let pid = crate::apps::launch(&entry)?;
    let Some(hwnd) =
        crate::apps::wait_for_window(pid, &entry.name, std::time::Duration::from_millis(10_000))?
    else {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            format!(
                "launched '{}' (pid {pid}) but no window appeared within 10s",
                entry.name
            ),
        )
        .details(json!({ "pid": pid }))
        .retryable(false));
    };
    platform::window_info(hwnd)?;
    match resolve_input_target(session, args) {
        Ok(Some(resolved)) => Ok(Some(resolved)),
        _ => Ok(Some(hwnd)),
    }
}

fn clipboard_write(args: &Value) -> Result<ToolExecution, ToolError> {
    let text = required_str(args, "text")?;
    platform::clipboard_write(text)?;
    Ok(ToolExecution::wrote(
        json!({ "written": true, "length": text.len() }),
        DispatchState::Sent,
    ))
}

fn clipboard_clean() -> Result<ToolExecution, ToolError> {
    platform::clipboard_clean()?;
    Ok(ToolExecution::wrote(
        json!({ "cleaned": true }),
        DispatchState::Sent,
    ))
}

fn find_image(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let templates = required_str(args, "template_path")?;
    let capture = platform::capture_screen(optional_region(args))?;
    let threshold = args
        .get("threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.9)
        .clamp(0.0, 1.0) as f32;
    let mut matches = Vec::new();
    for template_path in templates
        .split('|')
        .filter(|value| !value.trim().is_empty())
    {
        let path = Path::new(template_path.trim());
        if !engine.config().is_allowed_image_path(path) {
            return Err(ToolError::new(
                ErrorCode::PolicyDenied,
                format!(
                    "template path is outside configured image roots: {}",
                    path.display()
                ),
            ));
        }
        let template = image::open(path)
            .map_err(|error| {
                ToolError::new(
                    ErrorCode::InvalidParams,
                    format!("template image could not be opened: {error}"),
                )
            })?
            .to_rgba8();
        let hit = locate::find_image(&capture.image, capture.screen_rect, &template, threshold);
        matches.push(json!({
            "template_path": path.display().to_string(),
            "found": hit.found,
            "x": hit.x,
            "y": hit.y,
            "width": hit.width,
            "height": hit.height,
            "confidence": hit.confidence,
            "diagnostic": hit.diagnostic
        }));
    }
    let best = matches
        .iter()
        .filter(|value| value["found"].as_bool() == Some(true))
        .max_by(|left, right| {
            left["confidence"]
                .as_f64()
                .partial_cmp(&right["confidence"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();
    Ok(ToolExecution::read(json!({
        "found": best.is_some(),
        "best": best,
        "matches": matches,
        "region": capture.screen_rect
    })))
}

fn find_color(args: &Value) -> Result<ToolExecution, ToolError> {
    let color = locate::parse_color(required_str(args, "color")?)?;
    let capture = platform::capture_screen(optional_region(args))?;
    let direction = args
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("left_top");
    let hit = locate::find_color(&capture.image, capture.screen_rect, color, direction);
    Ok(ToolExecution::read(json!({
        "found": hit.is_some(),
        "x": hit.map(|point| point.0),
        "y": hit.map(|point| point.1),
        "region": capture.screen_rect
    })))
}

fn find_multi_color(args: &Value) -> Result<ToolExecution, ToolError> {
    let anchor = locate::parse_color(required_str(args, "anchor")?)?;
    let offsets = parse_offsets(required_str(args, "offsets")?)?;
    if offsets.is_empty() {
        return Err(invalid("offsets must not be empty"));
    }
    let capture = platform::capture_screen(optional_region(args))?;
    let min_ratio = args
        .get("min_match_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.8)
        .clamp(0.0, 1.0);
    let image = &capture.image;
    for y in 0..image.height() {
        for x in 0..image.width() {
            if !pixel_matches(image.get_pixel(x, y), anchor) {
                continue;
            }
            let mut matched = 0usize;
            for offset in &offsets {
                let ox = i64::from(x) + i64::from(offset.dx);
                let oy = i64::from(y) + i64::from(offset.dy);
                if ox < 0
                    || oy < 0
                    || ox >= i64::from(image.width())
                    || oy >= i64::from(image.height())
                {
                    continue;
                }
                let pixel = image.get_pixel(ox as u32, oy as u32);
                let expected = pixel_matches(pixel, offset.color);
                if expected != offset.negate {
                    matched += 1;
                }
            }
            if matched as f64 / offsets.len() as f64 >= min_ratio {
                let sx = capture.screen_rect.x.saturating_add(x as i32);
                let sy = capture.screen_rect.y.saturating_add(y as i32);
                return Ok(ToolExecution::read(json!({
                    "found": true,
                    "x": sx,
                    "y": sy,
                    "matched": matched,
                    "total": offsets.len(),
                    "region": capture.screen_rect
                })));
            }
        }
    }
    Ok(ToolExecution::read(json!({
        "found": false,
        "x": Value::Null,
        "y": Value::Null,
        "region": capture.screen_rect
    })))
}

fn find_text(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let dict_name = required_str(args, "dict_name")?;
    if dict_name.contains("..")
        || dict_name.contains('/')
        || dict_name.contains('\\')
        || Path::new(dict_name).is_absolute()
    {
        return Err(ToolError::new(
            ErrorCode::PolicyDenied,
            "dict_name must be a basename without path separators",
        ));
    }
    let words = required_str(args, "words")?;
    let region = optional_region(args);
    let capture = platform::capture_screen(region)?;
    let elements = engine
        .vision()
        .perceive(&capture.image, Some(capture.screen_rect))?;
    let targets: Vec<&str> = words
        .split('|')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .collect();
    let mut matches = Vec::new();
    for element in &elements {
        if targets.iter().any(|word| element.text.contains(word)) {
            let center = element.screen_center.unwrap_or((
                capture.screen_rect.x + element.rect.x + element.rect.width as i32 / 2,
                capture.screen_rect.y + element.rect.y + element.rect.height as i32 / 2,
            ));
            matches.push(json!({
                "word": crate::untrusted::wrap(&element.text),
                "x": center.0,
                "y": center.1,
                "width": element.rect.width,
                "height": element.rect.height,
                "confidence": element.confidence,
                "dict": dict_name
            }));
        }
    }
    let found = !matches.is_empty();
    Ok(ToolExecution::read(json!({
        "found": found,
        "match_count": matches.len(),
        "matches": matches,
        "region": capture.screen_rect,
        "engine": "paddle_ocr",
        "fallback_hint": if found { Value::Null } else {
            json!("OCR found no text containing the requested words in this region")
        }
    })))
}

/// Validation-only hook for policy tests that must not touch the desktop.
#[doc(hidden)]
pub fn execute_sync_for_test(
    session: &Session,
    tool: ToolName,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    let _ = session;
    match tool {
        ToolName::FindText => {
            let dict_name = required_str(args, "dict_name")?;
            if dict_name.contains("..")
                || dict_name.contains('/')
                || dict_name.contains('\\')
                || Path::new(dict_name).is_absolute()
            {
                return Err(ToolError::new(
                    ErrorCode::PolicyDenied,
                    "dict_name must be a basename without path separators",
                ));
            }
            Err(ToolError::new(
                ErrorCode::ModelUnavailable,
                "validation-only test hook",
            ))
        }
        _ => Err(invalid(
            "execute_sync_for_test only supports validation-only tools",
        )),
    }
}

async fn wait_for(args: &Value, cancel: &CancelFlag) -> Result<ToolExecution, ToolError> {
    let condition = required_str(args, "condition")?;
    if !matches!(condition, "change" | "stable" | "color") {
        return Err(invalid("condition must be change, stable or color"));
    }
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(10_000)
        .clamp(1, 300_000);
    let interval_ms = args
        .get("interval_ms")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(50, 5_000);
    let stable_ms = args
        .get("stable_ms")
        .and_then(Value::as_u64)
        .unwrap_or(500)
        .min(300_000);
    let region = optional_region(args);
    let color = args
        .get("color")
        .and_then(Value::as_str)
        .map(locate::parse_color)
        .transpose()?;
    if condition == "color" && color.is_none() {
        return Err(invalid("color is required for condition=color"));
    }
    let started = Instant::now();
    let baseline = platform::capture_screen(region)?;
    let mut previous = baseline.image.clone();
    let mut stable_since = Instant::now();
    let mut polls = 0u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(ToolError::new(ErrorCode::Cancelled, "wait was cancelled"));
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Ok(ToolExecution::read(json!({
                "met": false,
                "condition": condition,
                "polls": polls,
                "elapsed_ms": started.elapsed().as_millis()
            })));
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        polls += 1;
        let current = platform::capture_screen(region)?;
        let met = match condition {
            "change" => images_differ(&baseline.image, &current.image),
            "stable" => {
                if images_differ(&previous, &current.image) {
                    stable_since = Instant::now();
                    false
                } else {
                    stable_since.elapsed() >= Duration::from_millis(stable_ms)
                }
            }
            "color" => color.is_some_and(|color| {
                locate::find_color(&current.image, current.screen_rect, color, "left_top").is_some()
            }),
            _ => false,
        };
        previous = current.image;
        if met {
            return Ok(ToolExecution::read(json!({
                "met": true,
                "condition": condition,
                "polls": polls,
                "elapsed_ms": started.elapsed().as_millis()
            })));
        }
    }
}

/// 解析图像来源（capture_id 优先、或允许根内的 image_path），返回
/// （路径、capture_id、屏幕原点、像素图）。
fn load_tool_image(
    engine: &Engine,
    session: &Session,
    args: &Value,
) -> Result<
    (
        std::path::PathBuf,
        Option<String>,
        Option<ScreenRect>,
        image::RgbaImage,
    ),
    ToolError,
> {
    let capture_arg = args.get("capture_id").and_then(Value::as_str);
    let image_arg = args.get("image_path").and_then(Value::as_str);
    let (path, existing_id, screen_rect) = match (capture_arg, image_arg) {
        (Some(_), Some(_)) => {
            return Err(invalid("capture_id and image_path are mutually exclusive"))
        }
        (None, None) => return Err(invalid("image_path or capture_id is required")),
        (Some(id), None) => {
            let capture = session.capture(id)?;
            let rect = capture.screen_rect;
            (
                std::path::PathBuf::from(&capture.path),
                Some(id.to_string()),
                Some(rect),
            )
        }
        (None, Some(raw)) => {
            let path = normalize_image_path(engine, raw)?;
            let existing = session.find_capture_by_path(&path.to_string_lossy());
            let rect = existing.as_ref().map(|capture| capture.screen_rect);
            let id = existing.as_ref().map(|capture| capture.id.clone());
            (path, id, rect)
        }
    };
    let image = image::open(&path)
        .map_err(|error| {
            ToolError::new(
                ErrorCode::InvalidParams,
                format!("image could not be opened: {error}"),
            )
        })?
        .to_rgba8();
    Ok((path, existing_id, screen_rect, image))
}

fn perceive(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let text_only = args
        .get("text_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let min_confidence = args
        .get("min_confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if !(0.0..=1.0).contains(&min_confidence) {
        return Err(invalid("min_confidence must be between 0 and 1"));
    }
    let (path, capture_id, screen_rect, image) = load_tool_image(engine, session, args)?;
    let started = Instant::now();
    let (mut elements, cached) = if text_only {
        engine.vision().perceive_text_cached(&image, screen_rect)?
    } else {
        engine.vision().perceive_cached(&image, screen_rect)?
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    elements.retain(|element| f64::from(element.confidence) >= min_confidence);
    elements.sort_by(|left, right| {
        left.rect
            .y
            .cmp(&right.rect.y)
            .then(left.rect.x.cmp(&right.rect.x))
    });
    let text_count = elements
        .iter()
        .filter(|element| element.kind == "text" && !element.text.is_empty())
        .count();
    let lines = group_text_lines(&elements);
    let raw_text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let line_values = lines
        .iter()
        .map(|line| {
            json!({
                "text": crate::untrusted::wrap(&line.text),
                "rect": line.rect,
                "element_count": line.element_count
            })
        })
        .collect::<Vec<_>>();
    let line_count = line_values.len();
    let mut locations = std::collections::BTreeMap::new();
    let mut public = Vec::with_capacity(elements.len());
    for (index, element) in elements.into_iter().enumerate() {
        let id = u32::try_from(index).unwrap_or(u32::MAX);
        let screen_center = screen_rect.map(|origin| {
            (
                origin.x + element.rect.x + element.rect.width as i32 / 2,
                origin.y + element.rect.y + element.rect.height as i32 / 2,
            )
        });
        if capture_id.is_some() {
            let mut screen_rect_value = element.rect;
            if let Some(origin) = screen_rect {
                screen_rect_value.x += origin.x;
                screen_rect_value.y += origin.y;
            }
            locations.insert(
                id,
                ElementLocation {
                    id,
                    kind: element.kind.clone(),
                    text: element.text.clone(),
                    rect: screen_rect_value,
                    confidence: element.confidence,
                    screen_center,
                },
            );
        }
        public.push(json!({
            "element_id": id,
            "kind": element.kind,
            "text": crate::untrusted::wrap(&element.text),
            "rect": element.rect,
            "confidence": element.confidence,
            "screen_center": screen_center
        }));
    }
    if let Some(capture_id) = &capture_id {
        session.update_capture(capture_id, |capture| capture.elements = locations)?;
    }
    Ok(ToolExecution::read(json!({
        "capture_id": capture_id,
        "image_path": path.display().to_string(),
        "elements": public,
        "text_only": text_only,
        "min_confidence": min_confidence,
        "text_count": text_count,
        "raw_text": crate::untrusted::wrap(&raw_text),
        "lines": line_values,
        "line_count": line_count,
        "cached": cached,
        "elapsed_ms": elapsed_ms,
        "image_width": image.width(),
        "image_height": image.height(),
        "count": public.len()
    })))
}

#[derive(Debug)]
struct TextLine {
    text: String,
    rect: ScreenRect,
    element_count: usize,
}

/// Group OCR blocks that share a visual row into one line, then sort the lines
/// top-to-bottom and the blocks inside each line left-to-right.
fn group_text_lines(elements: &[crate::vision::VisionElement]) -> Vec<TextLine> {
    let mut text: Vec<&crate::vision::VisionElement> = elements
        .iter()
        .filter(|element| element.kind == "text" && !element.text.is_empty())
        .collect();
    text.sort_by(|left, right| {
        left.rect
            .y
            .cmp(&right.rect.y)
            .then(left.rect.x.cmp(&right.rect.x))
    });

    let mut groups: Vec<Vec<&crate::vision::VisionElement>> = Vec::new();
    for element in text {
        let mut placed = false;
        if let Some(group) = groups.last_mut() {
            if same_text_row(group[0], element) {
                group.push(element);
                placed = true;
            }
        }
        if !placed {
            groups.push(vec![element]);
        }
    }

    groups
        .into_iter()
        .map(|mut group| {
            group.sort_by(|left, right| {
                left.rect
                    .x
                    .cmp(&right.rect.x)
                    .then(left.rect.y.cmp(&right.rect.y))
            });
            let text = group
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let x = group.iter().map(|item| item.rect.x).min().unwrap_or(0);
            let y = group.iter().map(|item| item.rect.y).min().unwrap_or(0);
            let right = group
                .iter()
                .map(|item| item.rect.x + item.rect.width as i32)
                .max()
                .unwrap_or(x);
            let bottom = group
                .iter()
                .map(|item| item.rect.y + item.rect.height as i32)
                .max()
                .unwrap_or(y);
            TextLine {
                text,
                rect: ScreenRect {
                    x,
                    y,
                    width: (right - x).max(0) as u32,
                    height: (bottom - y).max(0) as u32,
                },
                element_count: group.len(),
            }
        })
        .collect()
}

/// Compare a block with the line anchor: prefer vertical overlap (robust when
/// one block is tall), with a small center-distance fallback. Anchoring on the
/// first block prevents a tall element from expanding the tolerance and
/// absorbing the next visual row.
fn same_text_row(
    anchor: &crate::vision::VisionElement,
    element: &crate::vision::VisionElement,
) -> bool {
    let anchor_top = anchor.rect.y;
    let anchor_bottom = anchor_top + anchor.rect.height as i32;
    let element_top = element.rect.y;
    let element_bottom = element_top + element.rect.height as i32;
    let overlap = (anchor_bottom.min(element_bottom) - anchor_top.max(element_top)).max(0);
    let min_height = (anchor.rect.height.min(element.rect.height) as i32).max(1);
    if overlap as f32 / min_height as f32 >= 0.5 {
        return true;
    }
    let anchor_center = anchor_top + anchor.rect.height as i32 / 2;
    let element_center = element_top + element.rect.height as i32 / 2;
    let tolerance = (anchor.rect.height.min(element.rect.height) as f32 * 0.5).max(4.0);
    (anchor_center - element_center).abs() as f32 <= tolerance
}

/// 把图像上的 OCR 文本按"标签 → 数值"配对（同行右侧优先，其次正下方），
/// 用于把老式表单/监控界面读成结构化键值数据（SAP/ERP/终端等）。
fn read_kv(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let (path, capture_id, _screen_rect, image) = load_tool_image(engine, session, args)?;
    let region = optional_region(args);
    let (image, offset) = match region {
        Some(rect) => {
            if rect.width == 0
                || rect.height == 0
                || rect.x < 0
                || rect.y < 0
                || i64::from(rect.x) + i64::from(rect.width) > i64::from(image.width())
                || i64::from(rect.y) + i64::from(rect.height) > i64::from(image.height())
            {
                return Err(invalid(format!(
                    "region is outside the image ({}x{})",
                    image.width(),
                    image.height()
                )));
            }
            (
                image::imageops::crop_imm(
                    &image,
                    rect.x as u32,
                    rect.y as u32,
                    rect.width,
                    rect.height,
                )
                .to_image(),
                (rect.x, rect.y),
            )
        }
        None => (image, (0, 0)),
    };
    let elements = engine.vision().perceive(&image, None)?;
    let kv_elements: Vec<crate::kv::KvElement> = elements
        .iter()
        .map(|element| crate::kv::KvElement {
            text: element.text.clone(),
            x: element.rect.x + offset.0,
            y: element.rect.y + offset.1,
            width: element.rect.width,
            height: element.rect.height,
            confidence: element.confidence,
        })
        .collect();
    let options = crate::kv::KvOptions {
        value_gap: args
            .get("value_gap")
            .and_then(Value::as_i64)
            .unwrap_or(600)
            .clamp(8, 4000) as i32,
        row_tolerance: args
            .get("row_tolerance")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 200) as i32,
        min_confidence: args
            .get("min_confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.3)
            .clamp(0.0, 1.0) as f32,
        max_pairs: args
            .get("max_pairs")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(1, 2000) as usize,
    };
    let (pairs, unpaired) = crate::kv::pair_key_values(&kv_elements, &options);
    let pairs_json: Vec<Value> = pairs
        .iter()
        .map(|pair| {
            json!({
                "label": crate::untrusted::wrap(&pair.label.text),
                "value": crate::untrusted::wrap(&pair.value.text),
                "label_rect": { "x": pair.label.x, "y": pair.label.y, "width": pair.label.width, "height": pair.label.height },
                "value_rect": { "x": pair.value.x, "y": pair.value.y, "width": pair.value.width, "height": pair.value.height },
                "row": pair.row,
                "ambiguous": pair.ambiguous
            })
        })
        .collect();
    let unpaired_json: Vec<Value> = unpaired
        .iter()
        .map(|element| {
            json!({
                "text": crate::untrusted::wrap(&element.text),
                "rect": { "x": element.x, "y": element.y, "width": element.width, "height": element.height },
                "confidence": element.confidence
            })
        })
        .collect();
    Ok(ToolExecution::read(json!({
        "image_path": path.display().to_string(),
        "capture_id": capture_id,
        "region": region.map(|rect| json!({
            "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height
        })),
        "pairs": pairs_json,
        "unpaired": unpaired_json,
        "pair_count": pairs_json.len(),
        "unpaired_count": unpaired_json.len(),
        "engine": "paddle_ocr"
    })))
}

/// 从已登记截图或允许目录中的图片裁剪一块区域，按截图策略编码保存；
/// capture 来源会登记为新的可链式 capture（元素坐标映射保持）。
fn image_crop(
    engine: &Engine,
    session: &Session,
    args: &Value,
) -> Result<ToolExecution, ToolError> {
    let x = required_i32(args, "x")?;
    let y = required_i32(args, "y")?;
    let width = required_u32(args, "width")?;
    let height = required_u32(args, "height")?;
    if width == 0 || height == 0 {
        return Err(invalid("width and height must be positive"));
    }
    let capture_id = args.get("capture_id").and_then(Value::as_str);
    let image_path = args.get("image_path").and_then(Value::as_str);
    let (source_image, source_label, source_capture) = match (capture_id, image_path) {
        (Some(_), Some(_)) => {
            return Err(invalid("capture_id and image_path are mutually exclusive"))
        }
        (None, None) => return Err(invalid("image_crop requires capture_id or image_path")),
        (Some(id), None) => {
            let capture = session.capture(id)?;
            let image = image::open(&capture.path)
                .map_err(|error| invalid(format!("capture image could not be opened: {error}")))?
                .to_rgba8();
            (image, json!({ "capture_id": id }), Some(capture))
        }
        (None, Some(raw)) => {
            let path = normalize_image_path(engine, raw)?;
            let image = image::open(&path)
                .map_err(|error| invalid(format!("image could not be opened: {error}")))?
                .to_rgba8();
            (
                image,
                json!({ "image_path": path.display().to_string() }),
                None,
            )
        }
    };
    let image_width = source_image.width();
    let image_height = source_image.height();
    if x < 0
        || y < 0
        || i64::from(x) + i64::from(width) > i64::from(image_width)
        || i64::from(y) + i64::from(height) > i64::from(image_height)
    {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            "region is outside the source image",
        )
        .details(json!({
            "image_width": image_width,
            "image_height": image_height,
            "region": { "x": x, "y": y, "width": width, "height": height }
        })));
    }
    let cropped =
        image::imageops::crop_imm(&source_image, x as u32, y as u32, width, height).to_image();
    let policy = crate::capture_policy::CapturePolicy::from_args(args, &engine.config().capture)?;
    let encoded = platform::save_capture_with_format(
        &cropped,
        args.get("path").and_then(Value::as_str),
        &engine.config().output_dir,
        &policy,
        args.get("format").and_then(Value::as_str),
    )?;
    let mut output_capture_id = Value::Null;
    if let Some(source) = source_capture {
        let screen_rect = ScreenRect {
            x: source.screen_rect.x + x,
            y: source.screen_rect.y + y,
            width,
            height,
        };
        let capture = platform::CapturedImage {
            image: cropped,
            screen_rect,
            foreground_hwnd: source.foreground_hwnd,
        };
        output_capture_id = json!(store_capture(
            session,
            &capture,
            encoded.path.to_string_lossy().into_owned()
        )?);
    }
    Ok(ToolExecution::read(json!({
        "capture_id": output_capture_id,
        "path": encoded.path.to_string_lossy(),
        "format": encoded.format.as_str(),
        "width": encoded.width,
        "height": encoded.height,
        "source": source_label,
        "region": { "x": x, "y": y, "width": width, "height": height },
        "source_width": image_width,
        "source_height": image_height,
        "bytes": encoded.bytes,
        "resized": encoded.resized
    })))
}

/// 把截图/文字/表格汇总为 `.docx`（纯本地文件操作，不抢前台）。
fn docx_save(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let sections = args
        .get("sections")
        .and_then(Value::as_array)
        .filter(|sections| !sections.is_empty())
        .ok_or_else(|| invalid("sections must be a non-empty array"))?;
    let mut blocks: Vec<crate::docx::Block> = Vec::new();
    if let Some(title) = args.get("title").and_then(Value::as_str) {
        blocks.push(crate::docx::Block::Title(title.to_string()));
    }
    let mut image_count = 0u32;
    for section in sections {
        if let Some(heading) = section.get("heading").and_then(Value::as_str) {
            blocks.push(crate::docx::Block::Heading(heading.to_string()));
        }
        if let Some(text) = section.get("text").and_then(Value::as_str) {
            blocks.push(crate::docx::Block::Paragraph(text.to_string()));
        }
        if let Some(image) = section.get("image").filter(|value| value.is_object()) {
            let (png, width_px, height_px) = load_document_image(engine, session, image)?;
            blocks.push(crate::docx::Block::Image {
                png,
                width_px,
                height_px,
                caption: image
                    .get("caption")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                width_cm: image.get("width_cm").and_then(Value::as_f64),
            });
            image_count += 1;
        }
        if let Some(table) = section.get("table").filter(|value| value.is_object()) {
            let rows = table
                .get("rows")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("table.rows must be an array"))?;
            let mut table_rows = Vec::new();
            for row in rows {
                let cells = row
                    .as_array()
                    .ok_or_else(|| invalid("table rows must be arrays"))?;
                let mut table_row = Vec::new();
                for cell in cells {
                    table_row.push(match cell {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    });
                }
                table_rows.push(table_row);
            }
            blocks.push(crate::docx::Block::Table(table_rows));
        }
    }
    blocks.push(crate::docx::Block::Paragraph(format!(
        "由 deskctl {} 生成于 {}",
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )));
    let options = crate::docx::DocumentOptions {
        header: args
            .get("header")
            .and_then(Value::as_str)
            .map(str::to_string),
        footer: args
            .get("footer")
            .and_then(Value::as_str)
            .map(str::to_string),
        page_numbers: args
            .get("page_numbers")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let path = match args.get("path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => std::path::PathBuf::from(path),
        _ => engine.config().output_dir.join(format!(
            "deskctl-report-{}.docx",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    };
    let bytes = crate::docx::write_document(&path, &blocks, &options)?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "bytes": bytes,
        "sections": sections.len(),
        "images": image_count,
        "title": args.get("title").and_then(Value::as_str),
        "header": options.header,
        "footer": options.footer,
        "page_numbers": options.page_numbers
    })))
}

/// 读取 `.docx` 的段落/表格/内嵌图片数量。
fn docx_read(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let path = normalize_image_path(engine, raw)?;
    let content = crate::docx::read_document(&path)?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "paragraphs": content.paragraphs,
        "tables": content.tables,
        "images": content.images,
        "headers": content.headers,
        "footers": content.footers,
        "truncated": content.truncated
    })))
}

/// 读取 `.xlsx`/`.xls`/`.ods` 的工作表与单元格值。
fn xlsx_read(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let path = normalize_image_path(engine, raw)?;
    let range = args
        .get("range")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(crate::xlsx::parse_range)
        .transpose()?;
    let sheets = crate::xlsx::read_workbook(&path, range)?;
    let filtered = if let Some(name) = args.get("sheet").and_then(Value::as_str) {
        let list: Vec<Value> = sheets
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|sheet| sheet["name"].as_str() == Some(name))
            .collect();
        if list.is_empty() {
            return Err(invalid(format!("sheet '{name}' not found")));
        }
        Value::Array(list)
    } else {
        sheets
    };
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "range": args.get("range").and_then(Value::as_str),
        "sheets": filtered
    })))
}

/// 写出 `.xlsx`（工作表名 + 行数据；字符串以 inline 方式存储）。
fn xlsx_save(args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let sheets_value = args
        .get("sheets")
        .and_then(Value::as_array)
        .filter(|sheets| !sheets.is_empty())
        .ok_or_else(|| invalid("sheets must be a non-empty array"))?;
    let mut sheets = Vec::new();
    for (index, sheet) in sheets_value.iter().enumerate() {
        let name = sheet
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Sheet{}", index + 1));
        let rows_value = sheet
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("sheet.rows must be an array"))?;
        let mut rows = Vec::new();
        for row in rows_value {
            let cells = row
                .as_array()
                .ok_or_else(|| invalid("sheet rows must be arrays"))?;
            rows.push(cells.clone());
        }
        let mut columns = Vec::new();
        if let Some(list) = sheet.get("columns").and_then(Value::as_array) {
            for column in list {
                if !column.is_object() {
                    return Err(invalid("sheet columns must be objects"));
                }
                columns.push(crate::xlsx::ColumnOptions {
                    width: column.get("width").and_then(Value::as_f64),
                    number_format: column
                        .get("number_format")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
        sheets.push(crate::xlsx::SheetSpec {
            name,
            rows,
            header_row: sheet
                .get("header_row")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            freeze_header: sheet
                .get("freeze_header")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            columns,
        });
    }
    let path = std::path::PathBuf::from(raw);
    let sheet_count = sheets.len();
    let bytes = crate::xlsx::write_workbook(&path, &sheets)?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "bytes": bytes,
        "sheets": sheet_count
    })))
}

/// 读取 `.pptx` 的每页文本与图片数量。
fn pptx_read(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let path = normalize_image_path(engine, raw)?;
    let presentation = crate::pptx::read_presentation(&path)?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "presentation": presentation
    })))
}

/// 写出 `.pptx`（每页标题 + 要点）。
fn pptx_save(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let slides_value = args
        .get("slides")
        .and_then(Value::as_array)
        .filter(|slides| !slides.is_empty())
        .ok_or_else(|| invalid("slides must be a non-empty array"))?;
    let mut slides = Vec::new();
    let mut image_count = 0u32;
    for slide in slides_value {
        let title = slide
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string);
        let bullets = slide
            .get("bullets")
            .and_then(Value::as_array)
            .map(|bullets| {
                bullets
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let layout = match slide.get("layout").and_then(Value::as_str) {
            None | Some("auto") => crate::pptx::SlideLayout::Auto,
            Some("title_bullets") => crate::pptx::SlideLayout::TitleBullets,
            Some("title_image") => crate::pptx::SlideLayout::TitleImage,
            Some("text_image") => crate::pptx::SlideLayout::TextImage,
            Some("title_only") => crate::pptx::SlideLayout::TitleOnly,
            Some(other) => {
                return Err(invalid(format!(
                    "unknown slide layout '{other}' (auto|title_bullets|title_image|text_image|title_only)"
                )))
            }
        };
        let image = match slide.get("image").filter(|value| value.is_object()) {
            Some(image) => {
                let (png, width_px, height_px) = load_document_image(engine, session, image)?;
                image_count += 1;
                Some(crate::pptx::SlideImage {
                    png,
                    width_px,
                    height_px,
                })
            }
            None => None,
        };
        slides.push(crate::pptx::SlideSpec {
            title,
            bullets,
            layout,
            image,
        });
    }
    let path = std::path::PathBuf::from(raw);
    let slide_count = slides.len();
    let bytes = crate::pptx::write_presentation(&path, &slides)?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "bytes": bytes,
        "slides": slide_count,
        "images": image_count
    })))
}

fn load_document_image(
    engine: &Engine,
    session: &Session,
    image: &Value,
) -> Result<(Vec<u8>, u32, u32), ToolError> {
    let capture_id = image.get("capture_id").and_then(Value::as_str);
    let image_path = image.get("image_path").and_then(Value::as_str);
    let decoded = match (capture_id, image_path) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "image.capture_id and image.image_path are mutually exclusive",
            ))
        }
        (None, None) => return Err(invalid("image requires capture_id or image_path")),
        (Some(id), None) => {
            let capture = session.capture(id)?;
            image::open(&capture.path)
                .map_err(|error| invalid(format!("capture image could not be opened: {error}")))?
        }
        (None, Some(raw)) => {
            let path = normalize_image_path(engine, raw)?;
            image::open(&path)
                .map_err(|error| invalid(format!("image could not be opened: {error}")))?
        }
    }
    .to_rgba8();
    let width_px = decoded.width();
    let height_px = decoded.height();
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(decoded)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|error| ToolError::new(ErrorCode::Internal, format!("encode png: {error}")))?;
    Ok((cursor.into_inner(), width_px, height_px))
}

fn targets_list(args: &Value) -> Result<ToolExecution, ToolError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let cursor = args.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let page_size = 50usize;
    let windows = platform::windows_list()?;
    let mut rows = Vec::new();
    for window in windows {
        let title = window["title"].as_str().unwrap_or_default();
        let process = window["process_name"].as_str().unwrap_or_default();
        if !query.is_empty()
            && !title.to_lowercase().contains(&query)
            && !process.to_lowercase().contains(&query)
        {
            continue;
        }
        let pid = window["process_id"].as_u64().unwrap_or_default();
        let hwnd = window["hwnd"].as_i64().unwrap_or_default();
        rows.push(json!({
            "app_ref": format!("app:{pid}:{}", sanitize_ref(process)),
            "window_ref": format!("win:{hwnd}"),
            "app_id": format!("{}:{pid}", sanitize_ref(process)),
            "kind": "window",
            "name": process,
            "title": title,
            "hwnd": hwnd,
            "position": { "x": window["x"], "y": window["y"] },
            "size": { "width": window["width"], "height": window["height"] },
            "foreground": window["foreground"]
        }));
    }
    if !query.is_empty() {
        let running: Vec<String> = platform::windows_list()?
            .iter()
            .filter_map(|window| window["process_name"].as_str().map(str::to_string))
            .collect();
        for entry in crate::apps::catalog() {
            if !entry.name.to_lowercase().contains(&query) {
                continue;
            }
            let running_now = entry
                .target_exe
                .as_deref()
                .is_some_and(|exe| running.iter().any(|name| name.eq_ignore_ascii_case(exe)));
            rows.push(json!({
                "app_ref": entry.app_ref,
                "window_ref": Value::Null,
                "app_id": entry.app_ref,
                "kind": "app",
                "name": entry.name,
                "title": Value::Null,
                "source": entry.source,
                "target_exe": entry.target_exe,
                "running": running_now
            }));
        }
    }
    rows.sort_by(|left, right| {
        left["name"]
            .as_str()
            .cmp(&right["name"].as_str())
            .then(left["title"].as_str().cmp(&right["title"].as_str()))
            .then(left["hwnd"].as_i64().cmp(&right["hwnd"].as_i64()))
    });
    let total = rows.len();
    let page: Vec<Value> = rows.into_iter().skip(cursor).take(page_size).collect();
    let next_cursor = (cursor + page.len() < total).then_some(cursor + page.len());
    Ok(ToolExecution::read(json!({
        "targets": page,
        "count": page.len(),
        "total": total,
        "next_cursor": next_cursor
    })))
}

fn target_bind(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let hwnd_arg = args.get("hwnd").and_then(Value::as_i64);
    let app_ref_arg = args.get("app_ref").and_then(Value::as_str);
    let window_ref = args.get("window_ref").and_then(Value::as_str);
    if hwnd_arg.is_none() && app_ref_arg.is_none() {
        return Err(invalid("target_bind requires hwnd, app_ref or window_ref"));
    }
    if hwnd_arg.is_some() && (app_ref_arg.is_some() || window_ref.is_some()) {
        return Err(invalid(
            "hwnd is mutually exclusive with app_ref/window_ref",
        ));
    }
    let app_ref = app_ref_arg.unwrap_or_default().to_string();
    let delivery_mode = args
        .get("delivery_mode")
        .and_then(Value::as_str)
        .unwrap_or("foreground");
    if !matches!(delivery_mode, "foreground" | "auto" | "background") {
        return Err(invalid(
            "delivery_mode must be foreground, auto or background",
        ));
    }
    let mut launched_pid: Option<u32> = None;
    let follow_dialog = args
        .get("follow_dialog")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut picked_dialog_parent: Option<i64> = None;
    let selected = if let Some(hwnd) = hwnd_arg {
        // 直接按 hwnd 绑定：无需先查 catalog / targets_list。
        platform::window_info(hwnd)?
    } else if let Some(entry) = crate::apps::find(&app_ref) {
        // 本地应用目录引用：运行中则绑定窗口，未运行则受限启动后绑定。
        resolve_catalog_target(entry, window_ref, &mut launched_pid)?
    } else {
        let (pid, app_name) = parse_app_ref(&app_ref)?;
        let windows = platform::windows_list()?;
        let matching: Vec<Value> = windows
            .into_iter()
            .filter(|window| {
                window["process_id"].as_u64() == Some(u64::from(pid))
                    && window["process_name"].as_str().is_some_and(|name| {
                        let sanitized = sanitize_ref(name);
                        sanitized == app_name
                            || sanitized
                                .strip_suffix(".exe")
                                .is_some_and(|stem| stem == app_name)
                    })
            })
            .collect();
        if let Some(window_ref) = window_ref {
            let hwnd = window_ref
                .strip_prefix("win:")
                .ok_or_else(|| invalid("window_ref must come from desktop_targets_list"))?
                .parse::<i64>()
                .map_err(|_| invalid("window_ref is malformed"))?;
            matching
                .into_iter()
                .find(|window| window["hwnd"].as_i64() == Some(hwnd))
                .ok_or_else(|| {
                    ToolError::new(
                        ErrorCode::TargetUnavailable,
                        "window_ref is not part of the selected app_ref",
                    )
                    .retryable(true)
                })?
        } else {
            let dialog_pick = if follow_dialog {
                pick_active_dialog(&matching)
            } else {
                None
            };
            match dialog_pick {
                Some((dialog, owner)) => {
                    picked_dialog_parent = Some(owner);
                    dialog
                }
                None => match matching.as_slice() {
                    [one] => one.clone(),
                    [] => {
                        return Err(ToolError::new(
                            ErrorCode::TargetUnavailable,
                            format!(
                                "no running window matches app_ref '{app_ref}' (expected app:<pid>:<process>, e.g. app:1234:saplogon.exe); use desktop_targets_list for current refs or pass hwnd directly"
                            ),
                        )
                        .retryable(true))
                    }
                    _ => {
                        let refs: Vec<String> = matching
                            .iter()
                            .map(|window| {
                                format!("win:{}", window["hwnd"].as_i64().unwrap_or_default())
                            })
                            .collect();
                        return Err(ToolError::new(
                            ErrorCode::TargetUnavailable,
                            format!(
                                "app_ref matches {} windows ({}); pass window_ref or hwnd",
                                refs.len(),
                                refs.join(", ")
                            ),
                        )
                        .details(json!({
                            "windows": matching.iter().map(|window| json!({
                                "window_ref": format!("win:{}", window["hwnd"].as_i64().unwrap_or_default()),
                                "title": window["title"]
                            })).collect::<Vec<_>>()
                        }))
                        .retryable(true))
                    }
                },
            }
        }
    };
    let mut hwnd = selected["hwnd"]
        .as_i64()
        .ok_or_else(|| invalid("selected window has no hwnd"))?;
    // follow_dialog：目标被模态对话框禁用时绑定该对话框（含显式 window_ref 的情况）。
    let mut parent_hwnd: Option<i64> = picked_dialog_parent;
    if follow_dialog {
        let effective = platform::active_input_window(hwnd)?;
        if effective != hwnd {
            parent_hwnd = Some(hwnd);
            hwnd = effective;
        }
    }
    let title = platform::window_info(hwnd)?["title"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let foreground = if delivery_mode == "background" {
        platform::window_info(hwnd)?["foreground"].as_bool() == Some(true)
    } else {
        platform::activate_window(hwnd)?
    };
    if delivery_mode == "foreground" && !foreground {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "target window could not be activated",
        )
        .retryable(true));
    }
    let app_ref = if app_ref.is_empty() {
        // hwnd 模式：用窗口信息合成一个稳定的 app_ref，便于后续响应/记录引用。
        let info = platform::window_info(hwnd)?;
        let pid = info["process_id"].as_u64().unwrap_or_default();
        let name = info["process_name"].as_str().unwrap_or("unknown");
        format!("app:{pid}:{}", sanitize_ref(name))
    } else {
        app_ref
    };
    let token = format!("target-{}", Uuid::new_v4().simple());
    session.store_target(TargetBinding {
        token: token.clone(),
        app_ref: app_ref.clone(),
        window_ref: window_ref.map(str::to_string),
        hwnd,
        title: title.clone(),
        created_at: Instant::now(),
    })?;
    Ok(ToolExecution::verified(
        json!({
            "target_token": token,
            "app_ref": app_ref,
            "window_ref": format!("win:{hwnd}"),
            "hwnd": hwnd,
            "title": title,
            "foreground": foreground,
            "delivery_mode": delivery_mode,
            "launched": launched_pid.is_some(),
            "pid": launched_pid,
            "followed_dialog": parent_hwnd.is_some(),
            "parent_hwnd": parent_hwnd
        }),
        DispatchState::Sent,
    ))
}

fn resolve_catalog_target(
    entry: crate::apps::AppEntry,
    window_ref: Option<&str>,
    launched_pid: &mut Option<u32>,
) -> Result<Value, ToolError> {
    let matching: Vec<Value> = platform::windows_list()?
        .into_iter()
        .filter(|window| window_matches_entry(window, &entry))
        .collect();
    if matching.is_empty() {
        // 应用未运行：受限启动（只接受 catalog 返回的引用，不接受任意路径）。
        let pid = crate::apps::launch(&entry)?;
        *launched_pid = Some(pid);
        let hwnd = crate::apps::wait_for_window(
            pid,
            &entry.name,
            std::time::Duration::from_millis(10_000),
        )?
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::TargetUnavailable,
                format!(
                    "launched '{}' (pid {pid}) but no window appeared within 10s",
                    entry.name
                ),
            )
            .details(json!({ "pid": pid }))
            .retryable(false)
        })?;
        return platform::window_info(hwnd);
    }
    if let Some(window_ref) = window_ref {
        let hwnd = window_ref
            .strip_prefix("win:")
            .ok_or_else(|| invalid("window_ref must come from desktop_targets_list"))?
            .parse::<i64>()
            .map_err(|_| invalid("window_ref is malformed"))?;
        return matching
            .into_iter()
            .find(|window| window["hwnd"].as_i64() == Some(hwnd))
            .ok_or_else(|| {
                ToolError::new(
                    ErrorCode::TargetUnavailable,
                    "window_ref is not part of the selected app_ref",
                )
                .retryable(true)
            });
    }
    match matching.as_slice() {
        [one] => Ok(one.clone()),
        [] => unreachable!("empty matching handled above"),
        _ => Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "app_ref has multiple windows; select one explicitly with window_ref",
        )
        .details(json!({
            "windows": matching.iter().map(|window| json!({
                "window_ref": format!("win:{}", window["hwnd"].as_i64().unwrap_or_default()),
                "title": window["title"]
            })).collect::<Vec<_>>()
        }))
        .retryable(true)),
    }
}

fn window_matches_entry(window: &Value, entry: &crate::apps::AppEntry) -> bool {
    if let (Some(exe), Some(name)) = (entry.target_exe.as_deref(), window["process_name"].as_str())
    {
        if name.eq_ignore_ascii_case(exe) {
            return true;
        }
    }
    window["title"]
        .as_str()
        .is_some_and(|title| title.to_lowercase().contains(&entry.name.to_lowercase()))
}

/// 在候选窗口中找出"被模态对话框禁用的窗口"的活动弹窗（弹窗必须也在候选中，
/// 即属于同一进程），用于 follow_dialog 场景。返回 (弹窗行, 被禁用的所有者 hwnd)。
fn pick_active_dialog(candidates: &[Value]) -> Option<(Value, i64)> {
    for window in candidates {
        let Some(hwnd) = window["hwnd"].as_i64() else {
            continue;
        };
        let Ok(effective) = platform::active_input_window(hwnd) else {
            continue;
        };
        if effective == hwnd {
            continue;
        }
        if let Some(row) = candidates
            .iter()
            .find(|row| row["hwnd"].as_i64() == Some(effective))
        {
            return Some((row.clone(), hwnd));
        }
    }
    None
}

fn store_capture(
    session: &Session,
    capture: &platform::CapturedImage,
    path: String,
) -> Result<String, ToolError> {
    let id = format!("cap-{}", Uuid::new_v4().simple());
    session.store_capture(CaptureState {
        id: id.clone(),
        path,
        screen_rect: capture.screen_rect,
        image_width: capture.image.width(),
        image_height: capture.image.height(),
        created_at: Instant::now(),
        elements: std::collections::BTreeMap::new(),
        foreground_hwnd: capture.foreground_hwnd,
    })?;
    Ok(id)
}

fn resolve_element(
    session: &Session,
    capture_id: &str,
    element_id: u32,
) -> Result<(i32, i32), ToolError> {
    resolve_element_location(session, capture_id, element_id)?
        .screen_center
        .ok_or_else(|| {
            ToolError::new(
                ErrorCode::StaleCapture,
                "element has no registered screen center",
            )
            .retryable(true)
        })
}

/// 解析元素绑定的完整位置信息（中心点 + 外接矩形），供"落点抖动"使用。
fn resolve_element_location(
    session: &Session,
    capture_id: &str,
    element_id: u32,
) -> Result<ElementLocation, ToolError> {
    let capture = session.capture(capture_id)?;
    if let Some(foreground) = capture.foreground_hwnd {
        if platform::foreground_hwnd().ok().flatten() != Some(foreground) {
            return Err(ToolError::new(
                ErrorCode::StaleCapture,
                "the foreground window changed since the capture; take a fresh screenshot",
            )
            .retryable(true));
        }
    }
    let element = capture.elements.get(&element_id).ok_or_else(|| {
        ToolError::new(
            ErrorCode::StaleCapture,
            format!("element {element_id} does not belong to capture {capture_id}; run desktop_perceive again"),
        )
        .retryable(true)
    })?;
    Ok(element.clone())
}

/// 元素内随机落点（内缩 2px）；元素过小时退回中心点。
fn jitter_in_rect(rng: &mut Pcg32, rect: &ScreenRect, fallback: (i32, i32)) -> (i32, i32) {
    let min_x = rect.x.saturating_add(2);
    let max_x = rect.x.saturating_add(rect.width as i32).saturating_sub(3);
    let min_y = rect.y.saturating_add(2);
    let max_y = rect.y.saturating_add(rect.height as i32).saturating_sub(3);
    if max_x < min_x || max_y < min_y {
        return fallback;
    }
    (
        rng.range_i64(min_x as i64, max_x as i64) as i32,
        rng.range_i64(min_y as i64, max_y as i64) as i32,
    )
}

/// 坐标定位的落点抖动：以点为 ±jitter_px 均匀偏移。
fn jitter_around(rng: &mut Pcg32, x: i32, y: i32, jitter_px: u64) -> (i32, i32) {
    if jitter_px == 0 {
        return (x, y);
    }
    let jitter = jitter_px as i64;
    (
        x.saturating_add(rng.range_i64(-jitter, jitter) as i32),
        y.saturating_add(rng.range_i64(-jitter, jitter) as i32),
    )
}

fn resolve_drag_point(
    session: &Session,
    args: &Value,
    capture_key: &str,
    element_key: &str,
    x_key: &str,
    y_key: &str,
) -> Result<(i32, i32), ToolError> {
    let capture = args.get(capture_key).and_then(Value::as_str);
    let element = args.get(element_key).and_then(Value::as_u64);
    let x = args.get(x_key).and_then(Value::as_i64);
    let y = args.get(y_key).and_then(Value::as_i64);
    match (capture, element, x, y) {
        (Some(capture), Some(element), None, None) => resolve_element(
            session,
            capture,
            u32::try_from(element).map_err(|_| invalid("element_id is too large"))?,
        ),
        (None, None, Some(x), Some(y)) => Ok((
            i32::try_from(x).map_err(|_| invalid("x is outside i32 range"))?,
            i32::try_from(y).map_err(|_| invalid("y is outside i32 range"))?,
        )),
        _ => Err(invalid(
            "each drag point must use capture_id + element_id or x + y, never a mix",
        )),
    }
}

fn resolve_input_target(session: &Session, args: &Value) -> Result<Option<i64>, ToolError> {
    let hwnd = args.get("hwnd").and_then(Value::as_i64);
    let locator = args.get("target_locator");
    if hwnd.is_some() && locator.is_some() {
        return Err(invalid("hwnd and target_locator are mutually exclusive"));
    }
    if let Some(hwnd) = hwnd {
        return Ok(Some(hwnd));
    }
    let Some(locator) = locator else {
        return Ok(None);
    };
    if let Some(window_id) = locator.get("window_id").and_then(Value::as_str) {
        if let Some(raw) = window_id.strip_prefix("hwnd:") {
            let hwnd = raw
                .parse::<i64>()
                .map_err(|_| invalid("window_id hwnd is invalid"))?;
            platform::window_info(hwnd)?;
            return Ok(Some(hwnd));
        }
    }
    if let Some(title) = locator.get("window_title").and_then(Value::as_str) {
        if let Some(hwnd) = platform::find_window_by_title(title)? {
            return Ok(Some(hwnd));
        }
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "target_locator did not resolve to a running window",
        )
        .retryable(true));
    }
    let target_token = locator
        .get("app_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("target_locator requires app_id, window_id or window_title"))?;
    let binding = session.target(target_token)?;
    Ok(Some(binding.hwnd))
}

fn ensure_foreground(hwnd: i64) -> Result<i64, ToolError> {
    // 模态对话框（原生 Save/Open 等）会禁用其所有者窗口；此时输入与焦点必须
    // 重定向到它的当前活动弹窗，否则会被错误地抢回主窗口。返回实际生效窗口。
    let hwnd = platform::active_input_window(hwnd)?;
    let info = platform::window_info(hwnd)?;
    if info["foreground"].as_bool() == Some(true) {
        return Ok(hwnd);
    }
    if platform::activate_window(hwnd)?
        || platform::window_info(hwnd)?["foreground"].as_bool() == Some(true)
    {
        Ok(hwnd)
    } else {
        Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "target window could not be brought to the foreground; no input was sent",
        )
        .retryable(true))
    }
}

fn optional_region(args: &Value) -> Option<ScreenRect> {
    args.get("region").and_then(|value| {
        Some(ScreenRect {
            x: i32::try_from(value.get("x")?.as_i64()?).ok()?,
            y: i32::try_from(value.get("y")?.as_i64()?).ok()?,
            width: u32::try_from(value.get("width")?.as_u64()?).ok()?,
            height: u32::try_from(value.get("height")?.as_u64()?).ok()?,
        })
    })
}

fn images_differ(left: &image::RgbaImage, right: &image::RgbaImage) -> bool {
    left.dimensions() != right.dimensions() || left.as_raw() != right.as_raw()
}

pub(crate) fn normalize_image_path(
    engine: &Engine,
    value: &str,
) -> Result<std::path::PathBuf, ToolError> {
    let path = Path::new(value);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !engine.config().is_allowed_image_path(&absolute) {
        let roots: Vec<String> = engine
            .config()
            .allowed_image_roots()
            .into_iter()
            .map(|root| root.display().to_string())
            .collect();
        return Err(ToolError::new(
            ErrorCode::PolicyDenied,
            format!(
                "image path '{}' is outside configured image roots ({}); move the file into one of these roots, set DESKCTL_ALLOWED_INPUT_ROOT, or pass capture_id when the image came from a deskctl capture",
                absolute.display(),
                roots.join(", ")
            ),
        )
        .retryable(false));
    }
    Ok(absolute)
}

/// 路径策略（与图片输入一致）：受配置的允许根约束；脚本文件与产物文件共用。
pub(crate) fn normalize_allowed_path(
    engine: &Engine,
    value: &str,
) -> Result<std::path::PathBuf, ToolError> {
    let path = Path::new(value);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !engine.config().is_allowed_image_path(&absolute) {
        let roots: Vec<String> = engine
            .config()
            .allowed_image_roots()
            .into_iter()
            .map(|root| root.display().to_string())
            .collect();
        return Err(ToolError::new(
            ErrorCode::PolicyDenied,
            format!(
                "path '{}' is outside the configured allowed roots ({}); move the file into one of these roots or set DESKCTL_ALLOWED_INPUT_ROOT",
                absolute.display(),
                roots.join(", ")
            ),
        )
        .retryable(false));
    }
    Ok(absolute)
}

/// `desktop_file_hash`：只读文件指纹（存在性 + sha256），受允许根策略约束。
fn file_hash(engine: &Engine, args: &Value) -> Result<ToolExecution, ToolError> {
    let raw = required_str(args, "path")?;
    let path = normalize_allowed_path(engine, raw)?;
    if !path.exists() {
        return Ok(ToolExecution::read(json!({
            "path": path.display().to_string(),
            "exists": false,
            "sha256": Value::Null,
            "bytes": Value::Null
        })));
    }
    let bytes = std::fs::read(&path).map_err(|error| {
        ToolError::new(
            ErrorCode::PermissionDenied,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    Ok(ToolExecution::read(json!({
        "path": path.display().to_string(),
        "exists": true,
        "sha256": crate::audit::digest_bytes(&bytes),
        "bytes": bytes.len(),
        "modified_ms": std::fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
    })))
}

struct OffsetColor {
    dx: i32,
    dy: i32,
    color: locate::Rgb,
    negate: bool,
}

fn parse_offsets(value: &str) -> Result<Vec<OffsetColor>, ToolError> {
    let mut offsets = Vec::new();
    for item in value.split(',').filter(|item| !item.trim().is_empty()) {
        let parts: Vec<&str> = item.split('|').collect();
        if parts.len() != 3 {
            return Err(invalid("offsets must use dx|dy|color[,dx|dy|color...]"));
        }
        let dx = parts[0]
            .parse::<i32>()
            .map_err(|_| invalid("offset dx is invalid"))?;
        let dy = parts[1]
            .parse::<i32>()
            .map_err(|_| invalid("offset dy is invalid"))?;
        let (negate, color) = if let Some(color) = parts[2].strip_prefix('!') {
            (true, locate::parse_color(color)?)
        } else {
            (false, locate::parse_color(parts[2])?)
        };
        offsets.push(OffsetColor {
            dx,
            dy,
            color,
            negate,
        });
    }
    Ok(offsets)
}

fn pixel_matches(pixel: &image::Rgba<u8>, color: locate::Rgb) -> bool {
    pixel[0].abs_diff(color.r) <= color.dr
        && pixel[1].abs_diff(color.g) <= color.dg
        && pixel[2].abs_diff(color.b) <= color.db
}

fn parse_app_ref(value: &str) -> Result<(u32, String), ToolError> {
    let mut parts = value.splitn(3, ':');
    if parts.next() != Some("app") {
        return Err(invalid("app_ref must come from desktop_targets_list"));
    }
    let pid = parts
        .next()
        .ok_or_else(|| invalid("app_ref is missing pid"))?
        .parse::<u32>()
        .map_err(|_| invalid("app_ref pid is invalid"))?;
    let name = parts
        .next()
        .ok_or_else(|| invalid("app_ref is missing app name"))?;
    Ok((pid, name.to_string()))
}

fn sanitize_ref(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn dispatch_error(mut error: ToolError, state: DispatchState) -> ToolError {
    error.details = json!({ "dispatch_state": state });
    error.retryable = false;
    error
}

pub(crate) fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{key} must be a non-empty string")))
}

fn required_i64(args: &Value, key: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("{key} must be an integer")))
}

fn optional_i64(args: &Value, key: &str) -> Result<Option<i64>, ToolError> {
    args.get(key)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| invalid(format!("{key} must be an integer")))
        })
        .transpose()
}

fn required_i32(args: &Value, key: &str) -> Result<i32, ToolError> {
    i32::try_from(required_i64(args, key)?)
        .map_err(|_| invalid(format!("{key} is outside i32 range")))
}

fn required_u32(args: &Value, key: &str) -> Result<u32, ToolError> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid(format!("{key} must be a non-negative 32-bit integer")))
}

pub(crate) fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_delay_ms_fixed_range_and_rejects_invalid() {
        assert_eq!(parse_delay_ms(Some(&json!(60))).unwrap(), Some((60, 60)));
        assert_eq!(
            parse_delay_ms(Some(&json!([30, 90]))).unwrap(),
            Some((30, 90))
        );
        assert!(parse_delay_ms(None).unwrap().is_none());
        assert!(parse_delay_ms(Some(&json!([90, 30]))).is_err());
        assert!(parse_delay_ms(Some(&json!([1, 2, 3]))).is_err());
        assert!(parse_delay_ms(Some(&json!("60"))).is_err());
        // 超上限必须拒绝并附范围（validation message 含上限值）。
        let error = parse_delay_ms(Some(&json!([1, 20_000]))).unwrap_err();
        assert!(error.message.contains("10000"), "{}", error.message);
    }

    #[test]
    fn group_text_lines_groups_rows_and_skips_icons() {
        let elements = vec![
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "第一行".to_string(),
                rect: ScreenRect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
                screen_center: None,
            },
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "同行".to_string(),
                rect: ScreenRect {
                    x: 20,
                    y: 1,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
                screen_center: None,
            },
            crate::vision::VisionElement {
                kind: "icon".to_string(),
                text: String::new(),
                rect: ScreenRect {
                    x: 20,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence: 0.8,
                screen_center: None,
            },
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "第二行".to_string(),
                rect: ScreenRect {
                    x: 0,
                    y: 20,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
                screen_center: None,
            },
        ];
        let lines = group_text_lines(&elements);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "第一行 同行");
        assert_eq!(lines[0].element_count, 2);
        assert_eq!(lines[1].text, "第二行");
        assert_eq!(lines[1].element_count, 1);
    }

    #[test]
    fn group_text_lines_keeps_next_row_after_a_tall_block() {
        let elements = vec![
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "A".to_string(),
                rect: ScreenRect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
                screen_center: None,
            },
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "B".to_string(),
                rect: ScreenRect {
                    x: 20,
                    y: 0,
                    width: 10,
                    height: 30,
                },
                confidence: 0.9,
                screen_center: None,
            },
            crate::vision::VisionElement {
                kind: "text".to_string(),
                text: "C".to_string(),
                rect: ScreenRect {
                    x: 0,
                    y: 20,
                    width: 10,
                    height: 10,
                },
                confidence: 0.9,
                screen_center: None,
            },
        ];
        let lines = group_text_lines(&elements);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "A B");
        assert_eq!(lines[1].text, "C");
    }

    #[test]
    #[cfg(windows)]
    fn key_spec_id_prefers_scancode_with_extended_bit() {
        let spec = platform::parse_key_spec("enter").unwrap();
        assert!(key_spec_id(&spec).starts_with("vk:"));
        let spec = platform::parse_key_spec("scan:0x1E").unwrap();
        assert_eq!(key_spec_id(&spec), "scan:0x1E");
        let spec = platform::parse_key_spec("scan:0xE01D").unwrap();
        assert_eq!(key_spec_id(&spec), "scan:0xE01D");
        let spec = platform::parse_key_spec("vk:0x41").unwrap();
        assert_eq!(key_spec_id(&spec), "vk:65");
    }

    #[test]
    fn jitter_stays_inside_rect_with_2px_inset() {
        let rect = ScreenRect {
            x: 100,
            y: 200,
            width: 40,
            height: 20,
        };
        let mut rng = humanize::Pcg32::new(7);
        for _ in 0..64 {
            let (x, y) = jitter_in_rect(&mut rng, &rect, (120, 210));
            assert!((102..=137).contains(&x), "x={x}");
            assert!((202..=217).contains(&y), "y={y}");
        }
    }

    #[test]
    fn jitter_falls_back_for_tiny_rects() {
        let rect = ScreenRect {
            x: 10,
            y: 10,
            width: 3,
            height: 3,
        };
        let mut rng = humanize::Pcg32::new(9);
        assert_eq!(jitter_in_rect(&mut rng, &rect, (11, 11)), (11, 11));
    }

    #[test]
    fn jitter_around_is_bounded_and_seed_deterministic() {
        let mut left = humanize::Pcg32::new(42);
        let mut right = humanize::Pcg32::new(42);
        let mut first = Vec::new();
        let mut second = Vec::new();
        for _ in 0..16 {
            first.push(jitter_around(&mut left, 500, 400, 5));
            second.push(jitter_around(&mut right, 500, 400, 5));
        }
        assert_eq!(first, second);
        for (x, y) in first {
            assert!((495..=505).contains(&x));
            assert!((395..=405).contains(&y));
        }
        // jitter_px=0 时原样返回。
        let mut rng = humanize::Pcg32::new(1);
        assert_eq!(jitter_around(&mut rng, 7, 8, 0), (7, 8));
    }

    #[test]
    fn mouse_button_arg_defaults_and_validates() {
        assert_eq!(mouse_button_arg(&json!({})).unwrap(), "left");
        assert_eq!(
            mouse_button_arg(&json!({"button": "right"})).unwrap(),
            "right"
        );
        assert!(mouse_button_arg(&json!({"button": "side"})).is_err());
    }
}
