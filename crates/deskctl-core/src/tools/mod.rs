mod semantic;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use deskctl_protocol::{DispatchState, Effect, ErrorCode, ToolError, ToolName, Verification};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::engine::{CancelFlag, Engine};
use crate::platform;
use crate::session::{CaptureState, ElementLocation, ScreenRect, Session, TargetBinding};
use crate::vision::locate;

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
    engine: &Engine,
    session: &Session,
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
        ToolName::Mouse => mouse(session, args),
        ToolName::MouseDrag => mouse_drag(session, args),
        ToolName::Input => input(session, args),
        ToolName::ClipboardWrite => clipboard_write(args),
        ToolName::ClipboardClean => clipboard_clean(),
        ToolName::FindImage => find_image(engine, args),
        ToolName::FindColor => find_color(args),
        ToolName::FindMultiColor => find_multi_color(args),
        ToolName::FindText => find_text(engine, args),
        ToolName::WaitFor => wait_for(args, cancel).await,
        ToolName::Perceive => perceive(engine, session, args),
        ToolName::Vision => Err(ToolError::new(
            ErrorCode::ModelUnavailable,
            "desktop_vision requires an external vision provider; none is configured",
        )
        .retryable(false)),
        ToolName::TargetsList => targets_list(args),
        ToolName::TargetBind => target_bind(session, args),
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
    let path = platform::save_png(
        &capture.image,
        args.get("path").and_then(Value::as_str),
        &engine.config().output_dir,
    )?;
    let capture_id = store_capture(session, &capture, path.clone())?;
    Ok(ToolExecution::read(json!({
        "capture_id": capture_id,
        "path": path,
        "width": capture.image.width(),
        "height": capture.image.height(),
        "screen_region": capture.screen_rect,
        "foreground_hwnd": capture.foreground_hwnd
    })))
}

fn window_activate(args: &Value) -> Result<ToolExecution, ToolError> {
    let hwnd = required_i64(args, "hwnd")?;
    let foreground = platform::activate_window(hwnd)?;
    if !foreground {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "window could not be brought to the foreground",
        )
        .retryable(true));
    }
    Ok(ToolExecution::verified(
        json!({ "hwnd": hwnd, "foreground": true }),
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
    let capture = platform::capture_window(hwnd)?;
    let path = platform::save_png(
        &capture.image,
        args.get("path").and_then(Value::as_str),
        &engine.config().output_dir,
    )?;
    let capture_id = store_capture(session, &capture, path.clone())?;
    Ok(ToolExecution::read(json!({
        "capture_id": capture_id,
        "path": path,
        "hwnd": hwnd,
        "width": capture.image.width(),
        "height": capture.image.height(),
        "screen_region": capture.screen_rect
    })))
}

fn mouse(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let action = required_str(args, "action")?;
    if action == "position" {
        let (x, y) = platform::mouse_position()?;
        return Ok(ToolExecution::read(json!({ "x": x, "y": y })));
    }
    if !matches!(
        action,
        "click" | "double_click" | "hover" | "move" | "scroll"
    ) {
        return Err(invalid("unknown mouse action"));
    }
    if let Some(hwnd) = optional_i64(args, "hwnd")? {
        ensure_foreground(hwnd)?;
    }
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
    let point = if has_capture {
        let capture_id = required_str(args, "capture_id")?;
        let element_id = required_u32(args, "element_id")?;
        resolve_element(session, capture_id, element_id)?
    } else if action == "scroll" {
        platform::mouse_position()?
    } else {
        (required_i32(args, "x")?, required_i32(args, "y")?)
    };
    match action {
        "click" | "double_click" => {
            let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
            if !matches!(button, "left" | "right" | "middle") {
                return Err(invalid("button must be left, right or middle"));
            }
            let clicks = if action == "double_click" {
                2
            } else {
                args.get("clicks").and_then(Value::as_u64).unwrap_or(1)
            };
            if !(1..=2).contains(&clicks) {
                return Err(invalid("clicks must be 1 or 2"));
            }
            platform::mouse_click(point.0, point.1, button, clicks as u8)?;
        }
        "hover" | "move" => platform::mouse_move(point.0, point.1)?,
        "scroll" => {
            let direction = args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down");
            let amount = args.get("amount").and_then(Value::as_u64).unwrap_or(3);
            platform::mouse_scroll(
                direction,
                u32::try_from(amount).map_err(|_| invalid("amount is too large"))?,
            )?;
        }
        _ => unreachable!(),
    }
    Ok(ToolExecution::wrote(
        json!({
            "action": action,
            "x": point.0,
            "y": point.1,
            "dispatch_state": "sent"
        }),
        DispatchState::Sent,
    ))
}

fn mouse_drag(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
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
    platform::drag(start, end, duration)?;
    Ok(ToolExecution::wrote(
        json!({
            "start": { "x": start.0, "y": start.1 },
            "end": { "x": end.0, "y": end.1 },
            "dispatch_state": "sent"
        }),
        DispatchState::Sent,
    ))
}

fn input(session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let mode = required_str(args, "mode")?;
    if !matches!(mode, "type" | "hotkey") {
        return Err(invalid("mode must be type or hotkey"));
    }
    let hwnd = resolve_input_target(session, args)?;
    if let Some(hwnd) = hwnd {
        ensure_foreground(hwnd)?;
    }
    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
    let send = args.get("send").and_then(Value::as_str).unwrap_or("enter");
    if !matches!(send, "enter" | "ctrl+enter" | "tab" | "none") {
        return Err(invalid("send must be enter, ctrl+enter, tab or none"));
    }
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
    if mode == "type" && text.is_empty() {
        return Err(invalid("text is required for mode=type"));
    }
    if mode == "hotkey" {
        let keys = keys.ok_or_else(|| invalid("keys are required for mode=hotkey"))?;
        if keys.is_empty() {
            return Err(invalid("keys must not be empty"));
        }
        for key in &keys {
            if !platform::is_valid_key(key) {
                return Err(invalid(format!("unknown key: {key}")));
            }
        }
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        platform::hotkey(&refs).map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
    } else {
        platform::type_text(text).map_err(|error| dispatch_error(error, DispatchState::Unknown))?;
        if send != "none" {
            let result = match send {
                "enter" => platform::hotkey(&["enter"]),
                "ctrl+enter" => platform::hotkey(&["ctrl", "enter"]),
                "tab" => platform::hotkey(&["tab"]),
                _ => Ok(()),
            };
            if let Err(error) = result {
                return Err(ToolError::new(
                    ErrorCode::DispatchUnknown,
                    format!("text was typed but the trailing key failed: {error}"),
                )
                .details(json!({ "dispatch_state": DispatchState::Partial }))
                .retryable(false));
            }
        }
    }
    Ok(ToolExecution::wrote(
        json!({ "mode": mode, "dispatch_state": "sent" }),
        DispatchState::Sent,
    ))
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
    if !engine.models().ocr_available() {
        return Err(ToolError::new(
            ErrorCode::ModelUnavailable,
            "OCR dictionary/model is unavailable; run deskctl doctor",
        )
        .retryable(false));
    }
    Err(ToolError::new(
        ErrorCode::ModelUnavailable,
        "find_text OCR backend is unavailable in this build",
    )
    .retryable(false))
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

fn perceive(engine: &Engine, session: &Session, args: &Value) -> Result<ToolExecution, ToolError> {
    let path = required_str(args, "image_path")?;
    let path = normalize_image_path(engine, path)?;
    let image = image::open(&path)
        .map_err(|error| {
            ToolError::new(
                ErrorCode::InvalidParams,
                format!("image could not be opened: {error}"),
            )
        })?
        .to_rgba8();
    let existing_capture = session.find_capture_by_path(&path.to_string_lossy());
    let screen_rect = existing_capture.as_ref().map(|capture| capture.screen_rect);
    let mut elements = engine.vision().perceive(&image, screen_rect)?;
    elements.sort_by(|left, right| {
        left.rect
            .y
            .cmp(&right.rect.y)
            .then(left.rect.x.cmp(&right.rect.x))
    });
    let capture_id = existing_capture.as_ref().map(|capture| capture.id.clone());
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
            "text": element.text,
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
        "count": public.len()
    })))
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
            "name": process,
            "title": title,
            "hwnd": hwnd,
            "position": { "x": window["x"], "y": window["y"] },
            "size": { "width": window["width"], "height": window["height"] },
            "foreground": window["foreground"]
        }));
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
    let app_ref = required_str(args, "app_ref")?;
    let window_ref = args.get("window_ref").and_then(Value::as_str);
    let (pid, app_name) = parse_app_ref(app_ref)?;
    let windows = platform::windows_list()?;
    let matching: Vec<Value> = windows
        .into_iter()
        .filter(|window| {
            window["process_id"].as_u64() == Some(u64::from(pid))
                && window["process_name"]
                    .as_str()
                    .is_some_and(|name| sanitize_ref(name) == app_name)
        })
        .collect();
    let selected = if let Some(window_ref) = window_ref {
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
        match matching.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(ToolError::new(
                    ErrorCode::TargetUnavailable,
                    "no running window matches app_ref; launch it outside Deskctl and list targets again",
                )
                .retryable(true))
            }
            _ => {
                return Err(ToolError::new(
                    ErrorCode::TargetUnavailable,
                    "app_ref has multiple windows; select one explicitly with window_ref",
                )
                .details(json!({
                    "windows": matching.iter().map(|window| json!({
                        "window_ref": format!("win:{}", window["hwnd"].as_i64().unwrap_or_default()),
                        "title": window["title"]
                    })).collect::<Vec<_>>()
                }))
                .retryable(true))
            }
        }
    };
    let hwnd = selected["hwnd"]
        .as_i64()
        .ok_or_else(|| invalid("selected window has no hwnd"))?;
    let title = selected["title"].as_str().unwrap_or_default().to_string();
    let foreground = platform::activate_window(hwnd)?;
    if args.get("delivery_mode").and_then(Value::as_str) == Some("foreground") && !foreground {
        return Err(ToolError::new(
            ErrorCode::TargetUnavailable,
            "target window could not be activated",
        )
        .retryable(true));
    }
    let token = format!("target-{}", Uuid::new_v4().simple());
    session.store_target(TargetBinding {
        token: token.clone(),
        app_ref: app_ref.to_string(),
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
            "foreground": foreground
        }),
        DispatchState::Sent,
    ))
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
    element.screen_center.ok_or_else(|| {
        ToolError::new(
            ErrorCode::StaleCapture,
            "element has no registered screen center",
        )
        .retryable(true)
    })
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

fn ensure_foreground(hwnd: i64) -> Result<(), ToolError> {
    let info = platform::window_info(hwnd)?;
    if info["foreground"].as_bool() == Some(true) {
        return Ok(());
    }
    if platform::activate_window(hwnd)?
        || platform::window_info(hwnd)?["foreground"].as_bool() == Some(true)
    {
        Ok(())
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

fn normalize_image_path(engine: &Engine, value: &str) -> Result<std::path::PathBuf, ToolError> {
    let path = Path::new(value);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !engine.config().is_allowed_image_path(&absolute) {
        return Err(ToolError::new(
            ErrorCode::PolicyDenied,
            "image path is outside configured image roots",
        )
        .retryable(false));
    }
    Ok(absolute)
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

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
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

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::InvalidParams, message)
}
