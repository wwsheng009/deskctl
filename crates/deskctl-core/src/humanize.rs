//! 拟人化输入采样：确定性 PCG32 + 鼠标轨迹/键盘节奏规划。
//!
//! 设计与验收见 `docs/design/input-simulation.md`（AC-MOUSE-012..014、
//! AC-INPUT-009..012）。所有随机采样都来自可播种的 PCG32：同一 seed +
//! 同一参数产生完全一致的序列，响应会回传实际使用的 `seed`，供审计、
//! 录放与脚本复现。

use deskctl_protocol::{ErrorCode, ToolError};
use serde_json::Value;

/// 轨迹点上限；超长距离自动放宽步长降采样（AC-MOUSE-013）。
pub const MAX_PATH_POINTS: usize = 2000;
/// 单次调用估算总时长上限；超限返回 `invalid_params` 并建议拆分。
pub const MAX_TOTAL_MS: u64 = 30_000;
/// 光标速度下限（物理像素/秒）。
pub const SPEED_MIN: f64 = 200.0;
/// 光标速度上限（物理像素/秒）。
pub const SPEED_MAX: f64 = 5000.0;
/// 光标速度默认值（物理像素/秒）。
pub const SPEED_DEFAULT: f64 = 1200.0;
/// 按下时长范围。
pub const HOLD_MS_MIN: u64 = 5;
pub const HOLD_MS_MAX: u64 = 5_000;
/// 重复次数范围。
pub const REPEAT_MIN: u64 = 1;
pub const REPEAT_MAX: u64 = 100;
/// 重复间隔范围。
pub const INTERVAL_MS_MIN: u64 = 10;
pub const INTERVAL_MS_MAX: u64 = 10_000;
/// 落点抖动上限（像素）。
pub const JITTER_PX_MAX: u64 = 200;
/// `type` 模式显式延迟上限（毫秒）。
pub const DELAY_MS_MAX: u64 = 10_000;

/// PCG32 伪随机序列：状态 64 位、输出 32 位，参考 pcg-random.org 的标准实现
/// （XSH-RR 输出函数、LCG 状态推进）。零依赖、可复现。
#[derive(Debug, Clone)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    /// 使用固定的流编号初始化，保证"同 seed 同序列"跨机器、跨版本一致。
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (0x853c_49e6_748f_ea9bu64 << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// 均匀分布在 `[0, 1)` 的浮点数。
    pub fn next_f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / (f64::from(u32::MAX) + 1.0)
    }

    /// 闭区间 `[lo, hi]` 的均匀整数（`hi <= lo` 时直接返回 `lo`）。
    pub fn range_u64(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + u64::from(self.next_u32()) % (hi - lo + 1)
    }

    /// 闭区间 `[lo, hi]` 的均匀整数。
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (u64::from(self.next_u32()) % ((hi - lo + 1) as u64)) as i64
    }

    /// 区间 `[lo, hi)` 的均匀浮点数。
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_f64() * (hi - lo)
    }

    /// 以概率 `probability` 返回 `true`。
    pub fn chance(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }
}

/// 拟人化强度：`off` 完全保持既有行为，`light` 轻弧度+轻微节奏，
/// `natural` 增加过冲回修与更明显的节奏变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanizeMode {
    Off,
    Light,
    Natural,
}

impl HumanizeMode {
    pub fn parse(value: Option<&Value>) -> Result<Self, ToolError> {
        let Some(value) = value else {
            return Ok(Self::Off);
        };
        if value.is_null() {
            return Ok(Self::Off);
        }
        match value.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("off") => Ok(Self::Off),
            Some("light") => Ok(Self::Light),
            Some("natural") => Ok(Self::Natural),
            _ => Err(ToolError::new(
                ErrorCode::InvalidParams,
                "humanize must be off, light or natural",
            )
            .retryable(false)),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Light => "light",
            Self::Natural => "natural",
        }
    }

    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}

fn range_error(name: &str, lo: u64, hi: u64) -> ToolError {
    ToolError::new(
        ErrorCode::InvalidParams,
        format!("{name} must be between {lo} and {hi}"),
    )
    .retryable(false)
}

/// 校验 `hold_ms`（5..=5000）。
pub fn validate_hold_ms(value: Option<&Value>) -> Result<Option<u64>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_u64()
        .ok_or_else(|| range_error("hold_ms", HOLD_MS_MIN, HOLD_MS_MAX))?;
    if !(HOLD_MS_MIN..=HOLD_MS_MAX).contains(&number) {
        return Err(range_error("hold_ms", HOLD_MS_MIN, HOLD_MS_MAX));
    }
    Ok(Some(number))
}

/// 校验 `repeat`（1..=100）。
pub fn validate_repeat(value: Option<&Value>) -> Result<Option<u64>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_u64()
        .ok_or_else(|| range_error("repeat", REPEAT_MIN, REPEAT_MAX))?;
    if !(REPEAT_MIN..=REPEAT_MAX).contains(&number) {
        return Err(range_error("repeat", REPEAT_MIN, REPEAT_MAX));
    }
    Ok(Some(number))
}

/// 校验 `interval_ms`（10..=10000）。
pub fn validate_interval_ms(value: Option<&Value>) -> Result<Option<u64>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_u64()
        .ok_or_else(|| range_error("interval_ms", INTERVAL_MS_MIN, INTERVAL_MS_MAX))?;
    if !(INTERVAL_MS_MIN..=INTERVAL_MS_MAX).contains(&number) {
        return Err(range_error("interval_ms", INTERVAL_MS_MIN, INTERVAL_MS_MAX));
    }
    Ok(Some(number))
}

/// 校验 `speed`（200..=5000 物理像素/秒）；未提供时返回 `None`（调用方决定默认）。
pub fn validate_speed(value: Option<&Value>) -> Result<Option<f64>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value.as_f64().ok_or_else(|| {
        ToolError::new(
            ErrorCode::InvalidParams,
            format!("speed must be a number between {SPEED_MIN} and {SPEED_MAX}"),
        )
        .retryable(false)
    })?;
    if !(SPEED_MIN..=SPEED_MAX).contains(&number) {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!("speed must be between {SPEED_MIN} and {SPEED_MAX} px/s"),
        )
        .retryable(false));
    }
    Ok(Some(number))
}

/// 校验 `jitter_px`（0..=200），默认 0。
pub fn validate_jitter_px(value: Option<&Value>) -> Result<u64, ToolError> {
    let Some(value) = value else {
        return Ok(0);
    };
    if value.is_null() {
        return Ok(0);
    }
    let number = value
        .as_u64()
        .ok_or_else(|| range_error("jitter_px", 0, JITTER_PX_MAX))?;
    if number > JITTER_PX_MAX {
        return Err(range_error("jitter_px", 0, JITTER_PX_MAX));
    }
    Ok(number)
}

/// 校验 `seed`（无符号 64 位），未提供时返回 `None`。
pub fn validate_seed(value: Option<&Value>) -> Result<Option<u64>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value.as_u64().map(Some).ok_or_else(|| {
        ToolError::new(
            ErrorCode::InvalidParams,
            "seed must be a non-negative integer",
        )
        .retryable(false)
    })
}

/// 生成默认随机 seed（时间纳秒 + 进程号，避免并发调用重复）。
pub fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ u64::from(std::process::id()).rotate_left(32)
}

/// 若 `seed` 为空则生成一个并回填，返回可复现的 RNG。
pub fn rng_with_seed(seed: &mut Option<u64>) -> Pcg32 {
    let value = seed.get_or_insert_with(random_seed);
    Pcg32::new(*value)
}

/// 可取消睡眠：按 25ms 分片轮询取消标志，保证 `request/cancel` 秒级生效
/// （AC-INPUT-012）。返回 `true` 表示完整睡完未被取消。
pub fn sleep_cancellable(total_ms: u64, cancel: &dyn Fn() -> bool) -> bool {
    let mut remaining = total_ms;
    while remaining > 0 {
        if cancel() {
            return false;
        }
        let chunk = remaining.min(25);
        std::thread::sleep(std::time::Duration::from_millis(chunk));
        remaining -= chunk;
    }
    !cancel()
}

/// 估算总时长超限时拒绝并建议拆分。
pub fn ensure_total_cap(total_ms: u64) -> Result<(), ToolError> {
    if total_ms > MAX_TOTAL_MS {
        return Err(ToolError::new(
            ErrorCode::InvalidParams,
            format!(
                "estimated duration {total_ms}ms exceeds the {MAX_TOTAL_MS}ms per-call limit; split the input into smaller calls"
            ),
        )
        .retryable(false));
    }
    Ok(())
}

/// 鼠标移动计划：`points` 为分段绝对坐标；`delays_ms[i]` 表示发送
/// `points[i]` 之后的停顿（最后一个点没有停顿），其长度等于
/// `points.len() - 1`。
#[derive(Debug, Clone, PartialEq)]
pub struct MovePlan {
    pub points: Vec<(i32, i32)>,
    pub delays_ms: Vec<u64>,
    pub total_ms: u64,
}

/// 生成拟人移动轨迹。
///
/// - `curved`：三次贝塞尔弧线（控制点垂直偏移 2%–15% 距离，模式下限不同）；
/// - `mode == Natural`：30% 概率终点外 1–3px 过冲后回修；
/// - 步长 1–3px 采样、ease-in-out 时间曲线、末端 2–4 个减速点；
/// - 超长距离自动放宽步长，点数不超过 [`MAX_PATH_POINTS`]。
pub fn plan_path(
    start: (i32, i32),
    end: (i32, i32),
    curved: bool,
    mode: HumanizeMode,
    speed: f64,
    rng: &mut Pcg32,
) -> MovePlan {
    let (sx, sy) = (f64::from(start.0), f64::from(start.1));
    let (ex, ey) = (f64::from(end.0), f64::from(end.1));
    let (dx, dy) = (ex - sx, ey - sy);
    let distance = (dx * dx + dy * dy).sqrt();
    if distance < 1.0 {
        return MovePlan {
            points: vec![end],
            delays_ms: Vec::new(),
            total_ms: 0,
        };
    }
    let (ux, uy) = (dx / distance, dy / distance);

    // 控制点垂直偏移：保证轨迹明显非直线（最小 1.5px）。
    let mut offset = 0.0f64;
    if curved && distance >= 8.0 {
        let (lo, hi) = match mode {
            HumanizeMode::Natural => (0.05, 0.15),
            _ => (0.02, 0.08),
        };
        let ratio = rng.range_f64(lo, hi);
        let sign = if rng.chance(0.5) { 1.0 } else { -1.0 };
        offset = ratio * distance * sign;
        if offset.abs() < 1.5 {
            offset = 1.5 * sign;
        }
    }
    let perp = (-uy, ux);
    let control = |factor: f64| {
        (
            sx + ux * distance * factor + perp.0 * offset,
            sy + uy * distance * factor + perp.1 * offset,
        )
    };
    let (c1x, c1y) = control(1.0 / 3.0);
    let (c2x, c2y) = control(2.0 / 3.0);
    let point_at = |t: f64| -> (f64, f64) {
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        (
            a * sx + b * c1x + c * c2x + d * ex,
            a * sy + b * c1y + c * c2y + d * ey,
        )
    };

    // 步长 1–3px 采样点数；预留 6 个点给减速/过冲。
    let step = rng.range_f64(1.0, 3.0);
    let raw = (distance / step).ceil().max(1.0);
    let cap = MAX_PATH_POINTS.saturating_sub(6).max(1) as f64;
    let n = raw.min(cap) as usize;
    let mut points: Vec<(i32, i32)> = Vec::with_capacity(n + 6);
    for index in 1..=n {
        let progress = index as f64 / n as f64;
        let eased = 0.5 - 0.5 * (std::f64::consts::PI * progress).cos();
        if eased >= 0.965 {
            break;
        }
        let (x, y) = point_at(eased);
        points.push((x.round() as i32, y.round() as i32));
    }
    let overshoot = mode == HumanizeMode::Natural && distance >= 30.0 && rng.chance(0.30);
    if overshoot {
        let extra = rng.range_u64(1, 3) as f64;
        points.push((
            (ex + ux * extra).round() as i32,
            (ey + uy * extra).round() as i32,
        ));
    } else {
        for t in [0.978, 0.995] {
            let (x, y) = point_at(t);
            points.push((x.round() as i32, y.round() as i32));
        }
    }
    points.push(end);
    points.dedup();

    let speed = speed.clamp(SPEED_MIN, SPEED_MAX);
    let mut delays_ms = Vec::with_capacity(points.len().saturating_sub(1));
    for pair in points.windows(2) {
        let step_x = f64::from(pair[1].0 - pair[0].0);
        let step_y = f64::from(pair[1].1 - pair[0].1);
        let step_distance = (step_x * step_x + step_y * step_y).sqrt();
        delays_ms.push((step_distance / speed * 1000.0).round() as u64);
    }
    let total_ms = delays_ms.iter().sum();
    MovePlan {
        points,
        delays_ms,
        total_ms,
    }
}

/// 拟人移动：`off` 直线、其余模式走弧线。
pub fn plan_move(
    start: (i32, i32),
    end: (i32, i32),
    mode: HumanizeMode,
    speed: f64,
    rng: &mut Pcg32,
) -> MovePlan {
    plan_path(start, end, mode.enabled(), mode, speed, rng)
}

/// 逐字输入计划：`chars` 与 `delays_ms` 一一对应，delay 表示发送该字符后的停顿。
#[derive(Debug, Clone, PartialEq)]
pub struct TypingPlan {
    pub chars: Vec<char>,
    pub delays_ms: Vec<u64>,
    pub total_ms: u64,
}

/// 判断制表符之外的标点（标点后节奏 +30%）。
pub fn is_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.' | ';' | ':' | '!' | '?' | '，' | '。' | '；' | '：' | '！' | '？' | '、' | '…'
    )
}

/// 生成逐字延迟序列。
///
/// - `explicit`：`delay_ms` 固定值或范围（`None` 时拟人默认 40–120ms）；
/// - `humanize` 开启时：标点后 +30%、空白后 5% 概率 200–400ms 单词停顿；
/// - 固定值模式（`explicit` 的 min == max 且 `humanize == off`）精确生效。
pub fn plan_typing(
    text: &str,
    mode: HumanizeMode,
    explicit: Option<(u64, u64)>,
    rng: &mut Pcg32,
) -> TypingPlan {
    let chars: Vec<char> = text.chars().collect();
    let mut delays_ms = Vec::with_capacity(chars.len());
    for ch in &chars {
        let mut delay = match explicit {
            Some((lo, hi)) if lo < hi => rng.range_u64(lo, hi),
            Some((lo, _)) => lo,
            None => rng.range_u64(40, 120),
        };
        if mode.enabled() {
            if is_punctuation(*ch) {
                delay = (delay as f64 * 1.3).round() as u64;
            }
            if ch.is_whitespace() && rng.chance(0.05) {
                delay = rng.range_u64(200, 400);
            }
        }
        delays_ms.push(delay);
    }
    let total_ms = delays_ms.iter().sum();
    TypingPlan {
        chars,
        delays_ms,
        total_ms,
    }
}

/// 鼠标按下时长采样：`off` 沿用 30ms；拟人 25–60ms。
pub fn sample_mouse_hold_ms(mode: HumanizeMode, rng: &mut Pcg32) -> u64 {
    match mode {
        HumanizeMode::Off => 30,
        HumanizeMode::Light | HumanizeMode::Natural => rng.range_u64(25, 60),
    }
}

/// 键盘按下时长采样：`off` 沿用 35ms；拟人 20–60ms。
pub fn sample_key_hold_ms(mode: HumanizeMode, rng: &mut Pcg32) -> u64 {
    match mode {
        HumanizeMode::Off => 35,
        HumanizeMode::Light | HumanizeMode::Natural => rng.range_u64(20, 60),
    }
}

/// 多击间隔采样：拟人 35–90ms，并受系统双击时间约束（留 20ms 余量）。
pub fn sample_click_gap_ms(mode: HumanizeMode, rng: &mut Pcg32, double_click_ms: u64) -> u64 {
    let raw = match mode {
        HumanizeMode::Off => 60,
        HumanizeMode::Light | HumanizeMode::Natural => rng.range_u64(35, 90),
    };
    raw.min(double_click_ms.saturating_sub(20)).max(25)
}

/// 移动到位后、按下前的停顿采样（20–90ms）。
pub fn sample_move_pause_ms(rng: &mut Pcg32) -> u64 {
    rng.range_u64(20, 90)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn pcg32_is_deterministic_per_seed() {
        let mut a = Pcg32::new(42);
        let mut b = Pcg32::new(42);
        let mut c = Pcg32::new(43);
        let seq_a: Vec<u32> = (0..16).map(|_| a.next_u32()).collect();
        let seq_b: Vec<u32> = (0..16).map(|_| b.next_u32()).collect();
        let seq_c: Vec<u32> = (0..16).map(|_| c.next_u32()).collect();
        assert_eq!(seq_a, seq_b);
        assert_ne!(seq_a, seq_c);
        assert!(seq_a.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn plan_path_is_deterministic_and_curved() {
        let mut a = Pcg32::new(7);
        let mut b = Pcg32::new(7);
        let plan_a = plan_path(
            (0, 0),
            (400, 0),
            true,
            HumanizeMode::Light,
            SPEED_DEFAULT,
            &mut a,
        );
        let plan_b = plan_path(
            (0, 0),
            (400, 0),
            true,
            HumanizeMode::Light,
            SPEED_DEFAULT,
            &mut b,
        );
        assert_eq!(plan_a.points, plan_b.points);
        assert_eq!(plan_a.delays_ms, plan_b.delays_ms);
        assert_eq!(plan_a.total_ms, plan_b.total_ms);
        assert!(plan_a.points.len() > 50);
        assert_eq!(*plan_a.points.last().unwrap(), (400, 0));
        let deviation = plan_a
            .points
            .iter()
            .map(|point| (point.1 as i64).abs())
            .max()
            .unwrap();
        assert!(deviation >= 1, "trajectory must not be perfectly straight");
    }

    #[test]
    fn plan_path_straight_line_stays_on_axis() {
        let mut rng = Pcg32::new(3);
        let plan = plan_path(
            (10, 10),
            (210, 10),
            false,
            HumanizeMode::Off,
            800.0,
            &mut rng,
        );
        assert!(plan.points.iter().all(|point| point.1 == 10));
        assert_eq!(*plan.points.last().unwrap(), (210, 10));
    }

    #[test]
    fn plan_path_respects_point_cap_and_delay_shape() {
        let mut rng = Pcg32::new(9);
        let plan = plan_path(
            (0, 0),
            (100_000, 0),
            true,
            HumanizeMode::Natural,
            SPEED_MAX,
            &mut rng,
        );
        assert!(plan.points.len() <= MAX_PATH_POINTS);
        assert_eq!(plan.delays_ms.len(), plan.points.len() - 1);
        assert_eq!(*plan.points.last().unwrap(), (100_000, 0));
        assert!(plan.total_ms > 0);
    }

    #[test]
    fn plan_typing_is_deterministic_and_ranged() {
        let text = "Hello, world! 你好。";
        let mut a = Pcg32::new(11);
        let mut b = Pcg32::new(11);
        let plan_a = plan_typing(text, HumanizeMode::Natural, None, &mut a);
        let plan_b = plan_typing(text, HumanizeMode::Natural, None, &mut b);
        assert_eq!(plan_a.delays_ms, plan_b.delays_ms);
        assert_eq!(plan_a.chars.len(), text.chars().count());
        assert_eq!(plan_a.delays_ms.len(), plan_a.chars.len());
        for delay in &plan_a.delays_ms {
            assert!(*delay >= 40 && *delay <= 400, "delay out of range: {delay}");
        }
        assert!(plan_a.delays_ms.iter().any(|delay| *delay > 120));
    }

    #[test]
    fn plan_typing_fixed_delay_is_exact() {
        let mut rng = Pcg32::new(5);
        let plan = plan_typing("abc", HumanizeMode::Off, Some((60, 60)), &mut rng);
        assert!(plan.delays_ms.iter().all(|delay| *delay == 60));
        let mut rng = Pcg32::new(5);
        let ranged = plan_typing("abcdef", HumanizeMode::Off, Some((30, 90)), &mut rng);
        assert!(ranged
            .delays_ms
            .iter()
            .all(|delay| (30..=90).contains(delay)));
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        assert!(validate_hold_ms(Some(&json!(3))).is_err());
        assert!(validate_hold_ms(Some(&json!(5_000))).is_ok());
        assert!(validate_hold_ms(None).unwrap().is_none());
        assert!(validate_repeat(Some(&json!(101))).is_err());
        assert!(validate_repeat(Some(&json!(0))).is_err());
        assert!(validate_interval_ms(Some(&json!(5))).is_err());
        assert!(validate_interval_ms(Some(&json!(10))).is_ok());
        assert!(validate_speed(Some(&json!(100.0))).is_err());
        assert!(validate_speed(Some(&json!(1200.0))).is_ok());
        assert!(validate_speed(None).unwrap().is_none());
        assert!(validate_seed(Some(&json!(-1))).is_err());
        assert_eq!(validate_seed(Some(&json!(7))).unwrap(), Some(7));
        assert!(validate_jitter_px(Some(&json!(201))).is_err());
        assert_eq!(validate_jitter_px(None).unwrap(), 0);
        assert!(ensure_total_cap(MAX_TOTAL_MS).is_ok());
        assert!(ensure_total_cap(MAX_TOTAL_MS + 1).is_err());
    }

    #[test]
    fn cancellable_sleep_stops_early() {
        let flag = Arc::new(AtomicBool::new(false));
        let setter = flag.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            setter.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let completed = sleep_cancellable(5_000, &|| flag.load(Ordering::Acquire));
        handle.join().unwrap();
        assert!(!completed);
        assert!(started.elapsed() < std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn sampling_ranges_follow_mode() {
        let mut rng = Pcg32::new(1);
        assert_eq!(sample_mouse_hold_ms(HumanizeMode::Off, &mut rng), 30);
        assert_eq!(sample_key_hold_ms(HumanizeMode::Off, &mut rng), 35);
        let mouse_hold = sample_mouse_hold_ms(HumanizeMode::Natural, &mut rng);
        assert!((25..=60).contains(&mouse_hold));
        let key_hold = sample_key_hold_ms(HumanizeMode::Light, &mut rng);
        assert!((20..=60).contains(&key_hold));
        let gap = sample_click_gap_ms(HumanizeMode::Natural, &mut rng, 500);
        assert!((35..=90).contains(&gap));
        let clamped = sample_click_gap_ms(HumanizeMode::Natural, &mut rng, 40);
        assert!((25..=40).contains(&clamped));
        let pause = sample_move_pause_ms(&mut rng);
        assert!((20..=90).contains(&pause));
    }

    #[test]
    fn humanize_mode_parses_known_values() {
        assert_eq!(HumanizeMode::parse(None).unwrap(), HumanizeMode::Off);
        assert_eq!(
            HumanizeMode::parse(Some(&json!("Natural"))).unwrap(),
            HumanizeMode::Natural
        );
        assert!(HumanizeMode::parse(Some(&json!("extreme"))).is_err());
    }
}
