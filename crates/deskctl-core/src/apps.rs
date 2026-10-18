//! Local application catalog and bounded launching.
//!
//! Catalog entries come only from fixed local shortcut roots (Start Menu + Desktop).
//! Callers reference entries by the opaque `app_ref` returned by the catalog;
//! raw executable or arbitrary paths are never accepted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use deskctl_protocol::ToolError;

/// 目录递归深度上限：防御目录联接等异常结构造成的环。
const MAX_SCAN_DEPTH: usize = 16;

/// 窗口等待的轮询间隔。
const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// 本地应用目录中的一个条目。
#[derive(Debug, Clone)]
pub struct AppEntry {
    /// "app:x:<16位hex>"，由规范化 .lnk 绝对路径的 sha256 前 16 hex 生成，稳定可复现。
    pub app_ref: String,
    /// .lnk 文件名 stem。
    pub name: String,
    /// "start_menu_user" | "start_menu_common" | "desktop_user" | "desktop_public"。
    pub source: &'static str,
    /// .lnk 绝对路径。
    pub shortcut: PathBuf,
    /// 经 IShellLinkW 解析出的目标可执行文件名（仅文件名），解析失败为 None。
    pub target_exe: Option<String>,
}

/// 固定扫描根：%APPDATA%\Microsoft\Windows\Start Menu\Programs、
/// %ProgramData%\Microsoft\Windows\Start Menu\Programs、
/// %USERPROFILE%\Desktop、%PUBLIC%\Desktop（不存在的根自动跳过）。
pub fn default_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    push_root(
        &mut roots,
        std::env::var_os("APPDATA"),
        &["Microsoft", "Windows", "Start Menu", "Programs"],
        "start_menu_user",
    );
    push_root(
        &mut roots,
        std::env::var_os("ProgramData"),
        &["Microsoft", "Windows", "Start Menu", "Programs"],
        "start_menu_common",
    );
    push_root(
        &mut roots,
        std::env::var_os("USERPROFILE"),
        &["Desktop"],
        "desktop_user",
    );
    push_root(
        &mut roots,
        std::env::var_os("PUBLIC"),
        &["Desktop"],
        "desktop_public",
    );
    roots
}

/// 拼接固定后缀并登记根目录；不存在的根自动跳过。
fn push_root(
    roots: &mut Vec<(PathBuf, &'static str)>,
    base: Option<std::ffi::OsString>,
    suffix: &[&str],
    source: &'static str,
) {
    let Some(base) = base else { return };
    let mut path = PathBuf::from(base);
    for part in suffix {
        path.push(part);
    }
    if path.is_dir() {
        roots.push((path, source));
    }
}

/// 递归扫描给定根下的 .lnk（只认 .lnk，大小写不敏感），按规范化路径去重，
/// 结果按 name 排序，总数上限 limit（默认调用请用 500）。单个 .lnk 解析失败
/// → target_exe=None，不 panic、不整体失败。
pub fn scan(roots: &[(PathBuf, &'static str)], limit: usize) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (root, source) in roots {
        collect_shortcuts(root, source, 0, &mut seen, &mut entries);
    }
    entries.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.shortcut.cmp(&right.shortcut))
    });
    entries.truncate(limit);
    entries
}

/// 递归收集单个根下的 .lnk 条目；读取失败（权限等）静默跳过。
fn collect_shortcuts(
    dir: &Path,
    source: &'static str,
    depth: usize,
    seen: &mut HashSet<String>,
    entries: &mut Vec<AppEntry>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read_dir.flatten() {
        let path = item.path();
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_shortcuts(&path, source, depth + 1, seen, entries);
            continue;
        }
        if !has_lnk_extension(&path) {
            continue;
        }
        let shortcut = normalize(&path);
        if !seen.insert(shortcut.to_string_lossy().into_owned()) {
            continue;
        }
        entries.push(AppEntry {
            app_ref: app_ref_for(&shortcut),
            name: file_stem(&shortcut),
            source,
            target_exe: imp::resolve_target_exe(&shortcut),
            shortcut,
        });
    }
}

/// 规范化路径：canonicalize 失败时退回原绝对路径。
fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// 只认 .lnk 扩展名（大小写不敏感）。
fn has_lnk_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("lnk"))
}

/// .lnk 文件名 stem。
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 不透明 app_ref：规范化绝对路径的 sha256 前 16 hex（小写）。
fn app_ref_for(shortcut: &Path) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(shortcut.to_string_lossy().as_bytes());
    let hex = hex::encode(digest);
    format!("app:x:{}", &hex[..16])
}

/// 完整目录：默认扫描根 + 500 条上限。
pub fn catalog() -> Vec<AppEntry> {
    scan(&default_roots(), 500)
}

/// 在给定条目集合中按 app_ref 查找。
pub fn find_in(entries: &[AppEntry], app_ref: &str) -> Option<AppEntry> {
    entries
        .iter()
        .find(|entry| entry.app_ref == app_ref)
        .cloned()
}

/// 在 catalog() 中按 app_ref 查找。
pub fn find(app_ref: &str) -> Option<AppEntry> {
    find_in(&catalog(), app_ref)
}

/// 受限启动：只接受目录内条目，对 .lnk 执行 ShellExecuteExW（verb "open"），
/// 返回新进程 pid；失败 → DispatchUnknown（retryable=false）。
pub fn launch(entry: &AppEntry) -> Result<u32, ToolError> {
    imp::launch(&entry.shortcut)
}

/// 轮询 crate::platform::windows_list()（每 250ms，最多 timeout）：优先 process_id == pid，
/// 其次标题包含 name_hint（忽略大小写）；命中返回 hwnd，超时 Ok(None)（不算错误）。
pub fn wait_for_window(
    pid: u32,
    name_hint: &str,
    timeout: Duration,
) -> Result<Option<i64>, ToolError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let windows = crate::platform::windows_list()?;
        if let Some(hwnd) = match_window(&windows, pid, name_hint) {
            return Ok(Some(hwnd));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(WINDOW_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }
}

/// 优先按 pid 匹配；其次按标题包含 name_hint（忽略大小写）匹配。
fn match_window(windows: &[serde_json::Value], pid: u32, name_hint: &str) -> Option<i64> {
    let by_pid = windows
        .iter()
        .find(|window| window["process_id"].as_u64() == Some(u64::from(pid)))
        .and_then(|window| window["hwnd"].as_i64());
    if by_pid.is_some() {
        return by_pid;
    }
    if name_hint.is_empty() {
        return None;
    }
    let needle = name_hint.to_lowercase();
    windows
        .iter()
        .find(|window| {
            window["title"]
                .as_str()
                .is_some_and(|title| title.to_lowercase().contains(&needle))
        })
        .and_then(|window| window["hwnd"].as_i64())
}

/// Windows 实现：COM 套间、IShellLinkW 解析与 ShellExecuteExW 启动。
#[cfg(windows)]
mod imp {
    use std::path::Path;

    use deskctl_protocol::{ErrorCode, ToolError};
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::Threading::GetProcessId;
    use windows::Win32::UI::Shell::{
        IShellLinkW, ShellExecuteExW, ShellLink, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS,
        SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// 与 platform::windows::uia 相同的 COM 套间约定：MTA 初始化；
    /// RPC_E_CHANGED_MODE 时复用已有套间且不反初始化。
    struct ComApartment(bool);

    impl ComApartment {
        fn initialize() -> Result<Self, String> {
            let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if result.is_ok() {
                Ok(Self(true))
            } else if result == RPC_E_CHANGED_MODE {
                Ok(Self(false))
            } else {
                Err(windows::core::Error::from(result).to_string())
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

    /// 经 IShellLinkW 解析 .lnk 目标路径，只保留可执行文件名；任何失败都返回 None。
    pub(super) fn resolve_target_exe(shortcut: &Path) -> Option<String> {
        let _com = ComApartment::initialize().ok()?;
        let path = wide(shortcut);
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistFile = link.cast().ok()?;
            persist.Load(PCWSTR(path.as_ptr()), STGM_READ).ok()?;
            let mut buffer = [0u16; 261];
            link.GetPath(&mut buffer, std::ptr::null_mut(), 0).ok()?;
            let length = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            let target = String::from_utf16_lossy(&buffer[..length]);
            let target = target.trim();
            if target.is_empty() {
                return None;
            }
            Path::new(target)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        }
    }

    /// ShellExecuteExW 启动 .lnk：verb "open"，SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI。
    /// 失败统一映射为 DispatchUnknown（retryable=false）；无论成败都关闭进程句柄。
    pub(super) fn launch(shortcut: &Path) -> Result<u32, ToolError> {
        let _com = ComApartment::initialize()
            .map_err(|message| dispatch_unknown(format!("initialize COM for launch: {message}")))?;
        let file = wide(shortcut);
        let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let exec = unsafe { ShellExecuteExW(&mut info) };
        let pid = unsafe { GetProcessId(info.hProcess) };
        // 无论成败都要关闭进程句柄，避免句柄泄漏。
        let _ = unsafe { CloseHandle(info.hProcess) };
        if let Err(error) = exec {
            return Err(dispatch_unknown(format!(
                "ShellExecuteExW failed for {}: {error}",
                shortcut.display()
            )));
        }
        if pid == 0 {
            return Err(dispatch_unknown(format!(
                "ShellExecuteExW returned no process for {}",
                shortcut.display()
            )));
        }
        Ok(pid)
    }

    /// 启动链路失败统一映射：DispatchUnknown 且 retryable(false)。
    fn dispatch_unknown(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::DispatchUnknown, message).retryable(false)
    }

    /// UTF-16 编码并以 NUL 结尾的路径缓冲。
    fn wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;

        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

/// 非 Windows 桩：目录/查找仍可用，解析与启动不可用。
#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use deskctl_protocol::{ErrorCode, ToolError};

    pub(super) fn resolve_target_exe(_shortcut: &Path) -> Option<String> {
        None
    }

    pub(super) fn launch(_shortcut: &Path) -> Result<u32, ToolError> {
        Err(ToolError::new(
            ErrorCode::PlatformNotSupported,
            "application launching requires Windows 10/11",
        )
        .retryable(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 测试根目录：结束（含 panic）时自动清理。
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("apps-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("create temp root");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 假 .lnk（解析必然失败）+ .txt（不应收录）+ 子目录中的大写 .LNK（递归 + 大小写）。
    fn seed(root: &Path) {
        fs::write(root.join("fake.lnk"), b"not a real shortcut").expect("write fake .lnk");
        fs::write(root.join("note.txt"), b"plain text").expect("write note.txt");
        let nested = root.join("sub");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(nested.join("Nested.LNK"), b"still not a shortcut").expect("write nested .LNK");
    }

    fn roots(root: &Path) -> Vec<(PathBuf, &'static str)> {
        vec![(root.to_path_buf(), "desktop_user")]
    }

    #[test]
    fn scan_collects_only_lnk_files() {
        let root = TempRoot::new();
        seed(root.path());
        let entries = scan(&roots(root.path()), 500);
        assert_eq!(entries.len(), 2, "只应收录 .lnk：{entries:?}");
        assert!(entries.iter().all(|entry| entry.source == "desktop_user"));
        assert!(entries.iter().any(|entry| entry.name == "fake"));
        assert!(entries.iter().any(|entry| entry.name == "Nested"));
        // 假 .lnk 解析失败：target_exe = None，不 panic、不整体失败。
        assert!(entries.iter().all(|entry| entry.target_exe.is_none()));
        // 结果按 name 排序。
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn app_ref_is_stable_and_opaque() {
        let root = TempRoot::new();
        seed(root.path());
        let first = scan(&roots(root.path()), 500);
        let second = scan(&roots(root.path()), 500);
        assert_eq!(first.len(), second.len());
        for entry in &first {
            let other = second
                .iter()
                .find(|candidate| candidate.name == entry.name)
                .expect("同名条目第二次扫描仍存在");
            assert_eq!(
                entry.app_ref, other.app_ref,
                "同一路径两次扫描 app_ref 必须一致"
            );
            let hex = entry.app_ref.strip_prefix("app:x:").expect("app_ref 前缀");
            assert_eq!(hex.len(), 16, "app_ref 应为 16 位 hex");
            assert!(
                hex.chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
                "app_ref 必须是小写 hex：{hex}"
            );
        }
    }

    #[test]
    fn find_in_rejects_unknown_refs() {
        let root = TempRoot::new();
        seed(root.path());
        let entries = scan(&roots(root.path()), 500);
        assert!(entries.len() >= 2);
        assert!(find_in(&entries, "app:x:0000000000000000").is_none());
        assert!(find_in(&entries, "app:x:nothex").is_none());
        assert!(find_in(&entries, "").is_none());
        let target = &entries[0];
        let found = find_in(&entries, &target.app_ref).expect("真实条目的 app_ref 必须可查找");
        assert_eq!(found.name, target.name);
        assert_eq!(found.shortcut, target.shortcut);
    }

    #[test]
    fn scan_respects_limit() {
        let root = TempRoot::new();
        seed(root.path());
        let entries = scan(&roots(root.path()), 1);
        assert_eq!(entries.len(), 1);
    }
}
