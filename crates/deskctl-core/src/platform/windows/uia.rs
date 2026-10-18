//! Bounded Windows UI Automation adapter.
//!
//! Native COM elements never leave this module. Callers receive redacted JSON
//! nodes with opaque IDs; execution re-observes the tree and re-resolves the
//! opaque ID rather than reusing a stale COM pointer.

use std::collections::{HashMap, VecDeque};

use deskctl_protocol::{ErrorCode, ToolError};
use serde_json::{json, Value};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, ExpandCollapseState_Expanded, IUIAutomation, IUIAutomation2,
    IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationRangeValuePattern, IUIAutomationScrollItemPattern, IUIAutomationScrollPattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, ScrollAmount_LargeDecrement, ScrollAmount_LargeIncrement,
    ScrollAmount_NoAmount, ScrollAmount_SmallDecrement, ScrollAmount_SmallIncrement,
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_ListControlTypeId,
    UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId, UIA_MenuControlTypeId,
    UIA_MenuItemControlTypeId, UIA_RadioButtonControlTypeId, UIA_RangeValuePatternId,
    UIA_ScrollItemPatternId, UIA_ScrollPatternId, UIA_SelectionItemPatternId, UIA_TabControlTypeId,
    UIA_TabItemControlTypeId, UIA_TogglePatternId, UIA_ValuePatternId, UIA_WindowControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow};

#[derive(Debug, Clone)]
pub struct UiNode {
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiaAction {
    Invoke,
    Toggle,
    SetChecked,
    Select,
    Expand,
    Collapse,
    Focus,
    SetValue,
    Scroll,
    ScrollIntoView,
    SetRangeValue,
}

/// 单次 UIA 动作的可选参数与派发约束。
#[derive(Debug, Clone)]
pub struct ActionOptions<'a> {
    /// set_value / set_range_value 的目标值。
    pub value: Option<&'a str>,
    /// set_checked 的目标勾选状态；缺省视为 true（幂等：已满足时不点击）。
    pub checked: Option<bool>,
    /// scroll 的方向：up / down / left / right。
    pub direction: Option<&'a str>,
    /// scroll 的幅度：small（默认）/ page。
    pub amount: Option<&'a str>,
    /// true 时要求目标窗口处于前台（默认）；background/auto 模式传 false。
    pub require_foreground: bool,
}

impl<'a> ActionOptions<'a> {
    pub fn new(value: Option<&'a str>) -> Self {
        Self {
            value,
            checked: None,
            direction: None,
            amount: None,
            require_foreground: true,
        }
    }
}

pub fn observe_window(hwnd: i64, limit: usize) -> Result<Vec<UiNode>, ToolError> {
    let _com = ComApartment::initialize()?;
    let automation = create_automation()?;
    let root = element_from_handle(&automation, hwnd)?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| uia_error("create UIA walker", error))?;
    let nodes = collect_nodes(&walker, root, hwnd, limit.clamp(1, 2000))?;
    Ok(nodes
        .into_iter()
        .map(|node| UiNode { value: node.public })
        .collect())
}

pub fn perform_action(
    hwnd: i64,
    node: &UiNode,
    action: UiaAction,
    options: &ActionOptions<'_>,
) -> Result<(), ToolError> {
    let _com = ComApartment::initialize()?;
    if options.require_foreground && unsafe { GetForegroundWindow() } != raw_handle(hwnd) {
        return Err(ToolError::new(
            ErrorCode::StaleObservation,
            "target window is no longer foreground",
        )
        .retryable(true));
    }
    let automation = create_automation()?;
    let root = element_from_handle(&automation, hwnd)?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| uia_error("create UIA walker", error))?;
    let nodes = collect_nodes(&walker, root, hwnd, 1000)?;
    let target = node
        .value
        .get("opaque_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new(ErrorCode::InvalidParams, "UI node has no opaque_id"))?;
    let matched: Vec<&NativeNode> = nodes
        .iter()
        .filter(|candidate| candidate.public["opaque_id"].as_str() == Some(target))
        .collect();
    let element = match matched.as_slice() {
        [one] => &one.element,
        [] => {
            return Err(ToolError::new(
                ErrorCode::StaleObservation,
                "UI element disappeared after re-observation",
            )
            .retryable(true))
        }
        _ => {
            return Err(ToolError::new(
                ErrorCode::StaleObservation,
                "UI element identity is ambiguous after re-observation",
            )
            .retryable(true))
        }
    };
    dispatch(element, action, options)
}

struct NativeNode {
    public: Value,
    element: IUIAutomationElement,
}

fn collect_nodes(
    walker: &IUIAutomationTreeWalker,
    root: IUIAutomationElement,
    hwnd: i64,
    limit: usize,
) -> Result<Vec<NativeNode>, ToolError> {
    let mut pending = VecDeque::from([(root, 0usize)]);
    let mut nodes = Vec::new();
    let mut ordinals: HashMap<String, usize> = HashMap::new();
    let started = std::time::Instant::now();
    while let Some((element, depth)) = pending.pop_front() {
        if nodes.len() >= limit || started.elapsed() > std::time::Duration::from_secs(8) {
            break;
        }
        if depth > 24 {
            continue;
        }
        let Ok(control_type) = (unsafe { element.CurrentControlType() }) else {
            continue;
        };
        let role = role_for(control_type);
        let name = text(unsafe { element.CurrentName() }.ok());
        let automation_id = text(unsafe { element.CurrentAutomationId() }.ok());
        let key = format!(
            "{}|{}|{}",
            role,
            automation_id.as_deref().unwrap_or_default(),
            name.as_deref().unwrap_or_default()
        );
        let ordinal = ordinals.entry(key.clone()).or_insert(0);
        let index = *ordinal;
        *ordinal += 1;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        key.hash(&mut hasher);
        let opaque_id = format!("uia:{:016x}:{}", hasher.finish(), index);

        let enabled = bool_or(unsafe { element.CurrentIsEnabled() }, false);
        let visible = !bool_or(unsafe { element.CurrentIsOffscreen() }, true);
        let focused = bool_or(unsafe { element.CurrentHasKeyboardFocus() }, false);
        let secure = bool_or(unsafe { element.CurrentIsPassword() }, false);
        let mut supported = Vec::new();
        if supports::<IUIAutomationInvokePattern>(&element, UIA_InvokePatternId) {
            supported.push("invoke");
        }
        let toggle = unsafe {
            element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
        }
        .ok();
        if toggle.is_some() {
            supported.push("toggle");
            supported.push("set_checked");
        }
        let selection = unsafe {
            element.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                UIA_SelectionItemPatternId,
            )
        }
        .ok();
        if selection.is_some() {
            supported.push("select");
        }
        let expand = unsafe {
            element.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                UIA_ExpandCollapsePatternId,
            )
        }
        .ok();
        let expanded = expand
            .as_ref()
            .and_then(|pattern| unsafe { pattern.CurrentExpandCollapseState() }.ok())
            .map(|state| state == ExpandCollapseState_Expanded);
        if let Some(expanded) = expanded {
            supported.push(if expanded { "collapse" } else { "expand" });
        }
        let value_pattern =
            unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
                .ok();
        if value_pattern.as_ref().is_some_and(|pattern| {
            unsafe { pattern.CurrentIsReadOnly() }
                .map(|value| !value.as_bool())
                .unwrap_or(false)
        }) {
            supported.push("set_value");
        }
        if bool_or(unsafe { element.CurrentIsKeyboardFocusable() }, false) {
            supported.push("focus");
        }
        if supports::<IUIAutomationScrollPattern>(&element, UIA_ScrollPatternId) {
            supported.push("scroll");
        }
        if supports::<IUIAutomationScrollItemPattern>(&element, UIA_ScrollItemPatternId) {
            supported.push("scroll_into_view");
        }
        let range_value = unsafe {
            element.GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId)
        }
        .ok();
        if range_value.as_ref().is_some_and(|pattern| {
            unsafe { pattern.CurrentIsReadOnly() }
                .map(|value| !value.as_bool())
                .unwrap_or(false)
        }) {
            supported.push("set_range_value");
        }
        let value_fingerprint = if secure {
            None
        } else {
            value_pattern
                .as_ref()
                .and_then(|pattern| unsafe { pattern.CurrentValue() }.ok())
                .map(|value| fingerprint(&value.to_string()))
        };
        let toggled = toggle
            .as_ref()
            .and_then(|pattern| unsafe { pattern.CurrentToggleState() }.ok())
            .and_then(toggle_boolean);
        let selected = selection
            .as_ref()
            .and_then(|pattern| unsafe { pattern.CurrentIsSelected() }.ok())
            .map(|value| value.as_bool());
        let element_for_children = element.clone();

        nodes.push(NativeNode {
            public: json!({
                "opaque_id": opaque_id,
                "role": role,
                "name": if secure { Value::Null } else { json!(name) },
                "automation_id": automation_id,
                "enabled": enabled,
                "visible": visible,
                "focused": focused,
                "secure": secure,
                "toggled": toggled,
                "selected": selected,
                "expanded": expanded,
                "short_value": Value::Null,
                "value_fingerprint": value_fingerprint,
                "supported_actions": supported,
                "window_id": format!("hwnd:{hwnd}"),
                "hwnd": hwnd
            }),
            element,
        });

        if let Ok(mut child) = unsafe { walker.GetFirstChildElement(&element_for_children) } {
            loop {
                let next = unsafe { walker.GetNextSiblingElement(&child) }.ok();
                pending.push_back((child, depth + 1));
                match next {
                    Some(next) => child = next,
                    None => break,
                }
            }
        }
    }
    Ok(nodes)
}

fn dispatch(
    element: &IUIAutomationElement,
    action: UiaAction,
    options: &ActionOptions<'_>,
) -> Result<(), ToolError> {
    let result: windows::core::Result<()> = unsafe {
        match action {
            UiaAction::Invoke => element
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .and_then(|pattern| pattern.Invoke()),
            UiaAction::Toggle => element
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .and_then(|pattern| pattern.Toggle()),
            UiaAction::SetChecked => {
                let desired = options.checked.unwrap_or(true);
                match element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                {
                    Ok(pattern) => {
                        let current = pattern.CurrentToggleState().ok().and_then(toggle_boolean);
                        if current == Some(desired) {
                            // 幂等：目标状态已满足时不点击。
                            return Ok(());
                        }
                        pattern.Toggle()
                    }
                    Err(error) => Err(error),
                }
            }
            UiaAction::Select => element
                .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
                .and_then(|pattern| pattern.Select()),
            UiaAction::Expand => element
                .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .and_then(|pattern| pattern.Expand()),
            UiaAction::Collapse => element
                .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .and_then(|pattern| pattern.Collapse()),
            UiaAction::Focus => element.SetFocus(),
            UiaAction::SetValue => {
                let value = options.value.ok_or_else(|| {
                    ToolError::new(ErrorCode::InvalidParams, "set_value requires value")
                })?;
                let value = BSTR::from(value);
                element
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .and_then(|pattern| pattern.SetValue(&value))
            }
            UiaAction::Scroll => {
                let large = options.amount == Some("page");
                let (horizontal, vertical) = match options.direction {
                    Some("up") => (
                        ScrollAmount_NoAmount,
                        if large {
                            ScrollAmount_LargeDecrement
                        } else {
                            ScrollAmount_SmallDecrement
                        },
                    ),
                    Some("down") => (
                        ScrollAmount_NoAmount,
                        if large {
                            ScrollAmount_LargeIncrement
                        } else {
                            ScrollAmount_SmallIncrement
                        },
                    ),
                    Some("left") => (
                        if large {
                            ScrollAmount_LargeDecrement
                        } else {
                            ScrollAmount_SmallDecrement
                        },
                        ScrollAmount_NoAmount,
                    ),
                    Some("right") => (
                        if large {
                            ScrollAmount_LargeIncrement
                        } else {
                            ScrollAmount_SmallIncrement
                        },
                        ScrollAmount_NoAmount,
                    ),
                    _ => {
                        return Err(ToolError::new(
                            ErrorCode::InvalidParams,
                            "scroll requires direction up, down, left or right",
                        ))
                    }
                };
                element
                    .GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
                    .and_then(|pattern| pattern.Scroll(horizontal, vertical))
            }
            UiaAction::ScrollIntoView => element
                .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
                .and_then(|pattern| pattern.ScrollIntoView()),
            UiaAction::SetRangeValue => {
                let raw = options.value.ok_or_else(|| {
                    ToolError::new(ErrorCode::InvalidParams, "set_range_value requires value")
                })?;
                let number: f64 = raw.parse().map_err(|_| {
                    ToolError::new(
                        ErrorCode::InvalidParams,
                        "set_range_value value must be a number",
                    )
                })?;
                element
                    .GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId)
                    .and_then(|pattern| pattern.SetValue(number))
            }
        }
    };
    result.map_err(|error| {
        ToolError::new(
            ErrorCode::DispatchUnknown,
            format!("UIA action was sent but the provider returned an error: {error}"),
        )
        .details(json!({ "dispatch_state": "unknown" }))
        .retryable(false)
    })
}

fn create_automation() -> Result<IUIAutomation, ToolError> {
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| uia_error("create UI Automation client", error))?
    };
    if let Ok(timeouts) = automation.cast::<IUIAutomation2>() {
        unsafe {
            let _ = timeouts.SetConnectionTimeout(1000);
            let _ = timeouts.SetTransactionTimeout(1000);
        }
    }
    Ok(automation)
}

fn element_from_handle(
    automation: &IUIAutomation,
    hwnd: i64,
) -> Result<IUIAutomationElement, ToolError> {
    let hwnd = raw_handle(hwnd);
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return Err(
            ToolError::new(ErrorCode::TargetUnavailable, "window handle is not valid")
                .retryable(true),
        );
    }
    unsafe { automation.ElementFromHandle(hwnd) }
        .map_err(|error| uia_error("read UIA root element", error))
}

fn raw_handle(hwnd: i64) -> HWND {
    HWND(isize::try_from(hwnd).unwrap_or_default())
}

fn role_for(control_type: windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID) -> &'static str {
    match control_type {
        value if value == UIA_WindowControlTypeId => "window",
        value if value == UIA_ButtonControlTypeId => "button",
        value if value == UIA_EditControlTypeId => "text_field",
        value if value == UIA_CheckBoxControlTypeId => "check_box",
        value if value == UIA_RadioButtonControlTypeId => "radio_button",
        value if value == UIA_ListControlTypeId => "list",
        value if value == UIA_ListItemControlTypeId => "list_item",
        value if value == UIA_MenuControlTypeId || value == UIA_MenuBarControlTypeId => "menu",
        value if value == UIA_MenuItemControlTypeId => "menu_item",
        value if value == UIA_TabControlTypeId || value == UIA_TabItemControlTypeId => "tab",
        value if value == UIA_DocumentControlTypeId => "document",
        _ => "other",
    }
}

fn supports<T: Interface>(
    element: &IUIAutomationElement,
    pattern: windows::Win32::UI::Accessibility::UIA_PATTERN_ID,
) -> bool {
    unsafe { element.GetCurrentPatternAs::<T>(pattern) }.is_ok()
}

fn bool_or(
    result: windows::core::Result<windows::Win32::Foundation::BOOL>,
    fallback: bool,
) -> bool {
    result.map(|value| value.as_bool()).unwrap_or(fallback)
}

fn text(value: Option<BSTR>) -> Option<String> {
    value
        .map(|value| truncate(&value.to_string(), 256))
        .filter(|value| !value.trim().is_empty())
}

fn fingerprint(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("value:{:016x}", hasher.finish())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn toggle_boolean(state: windows::Win32::UI::Accessibility::ToggleState) -> Option<bool> {
    use windows::Win32::UI::Accessibility::{ToggleState_Off, ToggleState_On};
    if state == ToggleState_On {
        Some(true)
    } else if state == ToggleState_Off {
        Some(false)
    } else {
        None
    }
}

fn uia_error(context: &str, error: windows::core::Error) -> ToolError {
    ToolError::new(ErrorCode::PermissionDenied, format!("{context}: {error}")).retryable(true)
}

struct ComApartment(bool);

impl ComApartment {
    fn initialize() -> Result<Self, ToolError> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            Ok(Self(true))
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            Err(uia_error(
                "initialize COM for UI Automation",
                windows::core::Error::from(result),
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
