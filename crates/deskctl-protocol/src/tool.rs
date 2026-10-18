use serde::{Deserialize, Serialize};

use crate::result::RiskClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    Minimal,
    #[default]
    Standard,
    Full,
    Readonly,
}

impl ToolProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Full => "full",
            Self::Readonly => "readonly",
        }
    }
}

impl std::str::FromStr for ToolProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "minimal" => Ok(Self::Minimal),
            "standard" => Ok(Self::Standard),
            "full" => Ok(Self::Full),
            "readonly" => Ok(Self::Readonly),
            other => Err(format!(
                "unknown profile '{other}'; expected minimal/standard/full/readonly"
            )),
        }
    }
}

impl std::fmt::Display for ToolProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    Mouse,
    MouseDrag,
    Input,
    Screenshot,
    ScreenSize,
    WindowScreenshot,
    WindowsList,
    WindowActivate,
    WindowMove,
    WindowResize,
    WindowInfo,
    Vision,
    Perceive,
    FindImage,
    FindColor,
    FindMultiColor,
    FindText,
    WaitFor,
    ClipboardClean,
    ClipboardWrite,
    TargetsList,
    TargetBind,
    SemanticObserve,
    SemanticCandidate,
    SemanticExecute,
    SemanticAction,
    VerifyState,
    AgentStep,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolMeta {
    pub name: &'static str,
    pub description: &'static str,
    pub requires_session: bool,
    pub requires_observation: bool,
    pub read_only: bool,
    pub destructive: bool,
    pub risk: RiskClass,
    pub optional: bool,
}

impl ToolName {
    pub const ALL: [ToolName; 28] = [
        Self::Mouse,
        Self::MouseDrag,
        Self::Input,
        Self::Screenshot,
        Self::ScreenSize,
        Self::WindowScreenshot,
        Self::WindowsList,
        Self::WindowActivate,
        Self::WindowMove,
        Self::WindowResize,
        Self::WindowInfo,
        Self::Vision,
        Self::Perceive,
        Self::FindImage,
        Self::FindColor,
        Self::FindMultiColor,
        Self::FindText,
        Self::WaitFor,
        Self::ClipboardClean,
        Self::ClipboardWrite,
        Self::TargetsList,
        Self::TargetBind,
        Self::SemanticObserve,
        Self::SemanticCandidate,
        Self::SemanticExecute,
        Self::SemanticAction,
        Self::VerifyState,
        Self::AgentStep,
    ];

    pub fn as_str(self) -> &'static str {
        self.meta().name
    }

    pub fn meta(self) -> ToolMeta {
        match self {
            Self::Mouse => meta(
                "desktop_mouse",
                "Mouse click, double-click, hover, move, scroll, or read cursor position.",
                RiskClass::BoundedWrite,
                false,
                false,
                false,
            ),
            Self::MouseDrag => meta(
                "desktop_mouse_drag",
                "Drag the mouse from a start point to an end point.",
                RiskClass::BoundedWrite,
                false,
                false,
                false,
            ),
            Self::Input => meta(
                "desktop_input",
                "Type text or send a hotkey to the foreground or a target window.",
                RiskClass::BoundedWrite,
                false,
                false,
                false,
            ),
            Self::Screenshot => meta(
                "desktop_screenshot",
                "Capture the screen or a screen region and save it as PNG.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::ScreenSize => meta(
                "desktop_screen_size",
                "Read the primary monitor resolution.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::WindowScreenshot => meta(
                "desktop_window_screenshot",
                "Capture one window and save it as PNG.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::WindowsList => meta(
                "desktop_windows_list",
                "List visible windows and their process information.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::WindowActivate => meta(
                "desktop_window_activate",
                "Bring a window to the foreground.",
                RiskClass::Reversible,
                false,
                false,
                false,
            ),
            Self::WindowMove => meta(
                "desktop_window_move",
                "Move a window to a new position.",
                RiskClass::Reversible,
                false,
                false,
                false,
            ),
            Self::WindowResize => meta(
                "desktop_window_resize",
                "Resize a window.",
                RiskClass::Reversible,
                false,
                false,
                false,
            ),
            Self::WindowInfo => meta(
                "desktop_window_info",
                "Read detailed window bounds and process information.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::Vision => meta(
                "desktop_vision",
                "Analyze a screenshot with an optional external vision provider.",
                RiskClass::ReadOnly,
                true,
                false,
                true,
            ),
            Self::Perceive => meta(
                "desktop_perceive",
                "Locate OCR and YOLO elements in a screenshot.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::FindImage => meta(
                "desktop_find_image",
                "Find a template image on screen.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::FindColor => meta(
                "desktop_find_color",
                "Find a color in a screen region.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::FindMultiColor => meta(
                "desktop_find_multi_color",
                "Find a multi-point color pattern on screen.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::FindText => meta(
                "desktop_find_text",
                "Find text using a local character dictionary.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::WaitFor => meta(
                "desktop_wait_for",
                "Wait for a screen region to change, stabilize, or contain a color.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::ClipboardClean => meta(
                "desktop_clipboard_clean",
                "Clear the clipboard.",
                RiskClass::BoundedWrite,
                false,
                false,
                false,
            ),
            Self::ClipboardWrite => meta(
                "desktop_clipboard_write",
                "Write text to the clipboard.",
                RiskClass::BoundedWrite,
                false,
                false,
                false,
            ),
            Self::TargetsList => meta(
                "desktop_targets_list",
                "List running windows; a query also matches launchable app refs from the local shortcut catalog.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::TargetBind => meta(
                "desktop_target_bind",
                "Bind a running window, or launch a catalogued local app and bind its window, returning a target token.",
                RiskClass::Reversible,
                false,
                false,
                false,
            ),
            Self::SemanticObserve => meta(
                "desktop_semantic_observe",
                "Read the Windows UIA tree and candidate actions for a bound target; tree_cursor continues the native tree page, delivery_mode becomes the default for later actions.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::SemanticCandidate => meta(
                "desktop_semantic_candidate",
                "Read details and a replay locator for an observed candidate.",
                RiskClass::ReadOnly,
                true,
                true,
                false,
            ),
            Self::SemanticExecute => meta(
                "desktop_semantic_execute",
                "Execute an observation-bound semantic candidate; supports checked/direction/amount and completion_policy with inline expectation verification.",
                RiskClass::ExternalCommit,
                false,
                true,
                false,
            ),
            Self::SemanticAction => meta(
                "desktop_semantic_action",
                "Execute a saved semantic locator (invoke/toggle/set_checked/select/expand/collapse/focus/set_value/scroll/scroll_into_view/set_range_value) with delivery mode, optional launch_ref and inline expectation verification.",
                RiskClass::ExternalCommit,
                false,
                true,
                false,
            ),
            Self::VerifyState => meta(
                "desktop_verify_state",
                "Verify a semantic postcondition with bounded waiting and stable sampling; never repeats the action.",
                RiskClass::ReadOnly,
                true,
                false,
                false,
            ),
            Self::AgentStep => meta(
                "desktop_agent_step",
                "One bounded semantic decision step using an optional decision provider.",
                RiskClass::ExternalCommit,
                false,
                true,
                true,
            ),
        }
    }

    pub fn profile_allows(self, profile: ToolProfile) -> bool {
        match profile {
            ToolProfile::Full => true,
            ToolProfile::Standard => !self.meta().optional,
            ToolProfile::Minimal => matches!(
                self,
                Self::Screenshot
                    | Self::WindowScreenshot
                    | Self::WindowsList
                    | Self::WindowActivate
                    | Self::Mouse
                    | Self::Input
            ),
            ToolProfile::Readonly => {
                self.meta().read_only
                    && matches!(
                        self,
                        Self::Screenshot
                            | Self::ScreenSize
                            | Self::WindowScreenshot
                            | Self::WindowsList
                            | Self::WindowInfo
                            | Self::Vision
                            | Self::Perceive
                            | Self::FindImage
                            | Self::FindColor
                            | Self::FindMultiColor
                            | Self::FindText
                            | Self::WaitFor
                            | Self::TargetsList
                            | Self::SemanticObserve
                            | Self::SemanticCandidate
                            | Self::VerifyState
                    )
            }
        }
    }
}

impl std::str::FromStr for ToolName {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|tool| tool.as_str() == value)
            .ok_or_else(|| format!("unknown desktop tool '{value}'"))
    }
}

pub fn tool_names_for_profile(profile: ToolProfile) -> Vec<ToolName> {
    ToolName::ALL
        .into_iter()
        .filter(|tool| tool.profile_allows(profile))
        .collect()
}

pub fn is_known_tool(name: &str) -> bool {
    ToolName::ALL.iter().any(|tool| tool.as_str() == name)
}

pub fn tool_meta(name: &str) -> Option<ToolMeta> {
    ToolName::ALL
        .into_iter()
        .find(|tool| tool.as_str() == name)
        .map(ToolName::meta)
}

fn meta(
    name: &'static str,
    description: &'static str,
    risk: RiskClass,
    read_only: bool,
    destructive: bool,
    optional: bool,
) -> ToolMeta {
    let requires_session = matches!(
        name,
        "desktop_targets_list"
            | "desktop_target_bind"
            | "desktop_semantic_observe"
            | "desktop_semantic_candidate"
            | "desktop_semantic_execute"
            | "desktop_semantic_action"
            | "desktop_verify_state"
            | "desktop_agent_step"
    );
    let requires_observation = matches!(
        name,
        "desktop_semantic_candidate"
            | "desktop_semantic_execute"
            | "desktop_semantic_action"
            | "desktop_agent_step"
    );
    ToolMeta {
        name,
        description,
        requires_session,
        requires_observation,
        read_only,
        destructive,
        risk,
        optional,
    }
}
