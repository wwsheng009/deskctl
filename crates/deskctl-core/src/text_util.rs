//! 文本修复工具：处理第三方 UI provider 返回的双层编码乱码。

use encoding_rs::GBK;

/// 修复 UI provider 的双层编码乱码，支持两个方向：
///
/// - `UTF-8 字节被按 GBK 解码`（`鎸夐檷搴忔帓搴?` → `按降序排`）；
/// - `GBK 字节被按 UTF-8 解码`（`ϵͳ` → `系统`）。
///
/// 判定只看"修复结果是否像正常文本"：结果必须由 ASCII/CJK/假名/谚文/全角组成，
/// 方向 1 还要求明显变短（乱码按 2 字节编码，原文按 3 字节，比例约 1.5×）。
/// 因此对正常中文/英文是恒等操作（已用常见词回归覆盖）。
pub fn repair_cjk_mojibake(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    // 方向 1：把字符串按 GBK 重新编码，若字节恰好是合法 UTF-8，说明原串是
    // UTF-8→GBK 的乱码。
    // 末位残字节会变成替换符，SAP 等还会加省略号后缀，一并去掉再重编码。
    let stripped: String = value
        .chars()
        .filter(|ch| *ch != '\u{FFFD}')
        .collect::<String>()
        .trim_end_matches(['.', '\u{2026}', '\u{00B7}', ' '])
        .to_string();
    if !stripped.is_empty() {
        if let Some(repaired) = gbk_encode_then_utf8(&stripped) {
            let shorter = repaired.chars().count() * 5 <= stripped.chars().count() * 4;
            if repaired != value && shorter && is_clean_text(&repaired) {
                return repaired;
            }
        }
    }
    // 方向 2：把字符串的 UTF-8 字节按 GBK 解码（仅当原串本身不像正常文本时）。
    if !is_clean_text(value) {
        let (decoded, _, had_errors) = GBK.decode(value.as_bytes());
        if !had_errors && decoded != value && is_clean_text(&decoded) {
            return decoded.to_string();
        }
    }
    value.to_string()
}

fn gbk_encode_then_utf8(value: &str) -> Option<String> {
    let (bytes, _, had_errors) = GBK.encode(value);
    if had_errors {
        return None;
    }
    match std::str::from_utf8(&bytes) {
        Ok(text) => Some(text.to_string()),
        Err(error) => {
            // 允许"末尾残留半个 UTF-8 字符"（原串末位不可恢复）：保留合法前缀。
            let valid = error.valid_up_to();
            if valid >= 3 && bytes.len().saturating_sub(valid) <= 3 {
                Some(String::from_utf8_lossy(&bytes[..valid]).to_string())
            } else {
                None
            }
        }
    }
}

/// 正常文本判定：ASCII、CJK（含扩展 A/兼容区）、假名、谚文、全角。
fn is_clean_text(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii()
                || matches!(ch as u32,
                    0x3000..=0x30FF   // CJK 标点与假名
                    | 0x3400..=0x4DBF // 扩展 A
                    | 0x4E00..=0x9FFF // 基本汉字
                    | 0xAC00..=0xD7AF // 谚文
                    | 0xF900..=0xFAFF // 兼容汉字
                    | 0xFF00..=0xFFEF // 全角
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_utf8_decoded_as_gbk() {
        let original = "按降序排序";
        // 模拟真实乱码：把 UTF-8 字节按 GBK 解码（末尾不足一组的字节变成 U+FFFD）。
        let (mojibake, _, had_errors) = GBK.decode(original.as_bytes());
        assert!(had_errors, "15 字节无法按 2 字节分组，应产生替换符");
        let mojibake = mojibake.to_string();
        assert_ne!(mojibake, original);
        assert_eq!(repair_cjk_mojibake(&mojibake), "按降序排");
    }

    #[test]
    fn repairs_gbk_decoded_as_utf8() {
        let original = "系统";
        // 模拟反向乱码：GBK 字节被按 UTF-8 解码成 "ϵͳ"。
        let mojibake = String::from_utf8(GBK.encode(original).0.to_vec()).expect("valid utf8");
        assert_ne!(mojibake, original);
        assert_eq!(repair_cjk_mojibake(&mojibake), original);
    }

    #[test]
    fn keeps_common_chinese_words_unchanged() {
        for word in [
            "系统",
            "按降序排序",
            "主机名",
            "可用内存",
            "物理内存",
            "页换入",
            "已消耗容量",
            "报告",
            "开始",
            "状态",
            "系统巡检",
            "命令字段",
            "保存",
            "取消",
        ] {
            assert_eq!(repair_cjk_mojibake(word), word, "误伤正常中文：{word}");
        }
    }

    #[test]
    fn keeps_ascii_and_other_scripts_unchanged() {
        assert_eq!(repair_cjk_mojibake("Password"), "Password");
        assert_eq!(repair_cjk_mojibake("OK Code"), "OK Code");
        assert_eq!(repair_cjk_mojibake("システム"), "システム");
        assert_eq!(repair_cjk_mojibake(""), "");
    }

    /// 模拟真实乱码：把 UTF-8 字节按 GBK 解码。
    fn simulate_gbk_mojibake(word: &str) -> String {
        GBK.decode(word.as_bytes()).0.to_string()
    }

    #[test]
    fn recovers_cjk_words_even_with_lossy_tail() {
        // 末尾残字节不可恢复（丢掉原文最后一个字），但其余部分必须还原。
        for (word, expected) in [
            ("设置过滤器", "设置过滤"),
            ("按升序排序", "按升序排"),
            ("按降序排序", "按降序排"),
            ("最终用户文档", "最终用户文"),
            ("主机名", "主机"),
            ("收藏夹", "收藏"),
            ("系统", "系统"),
        ] {
            let mojibake = simulate_gbk_mojibake(word);
            assert_ne!(mojibake, word, "模拟乱码应与原文不同：{word}");
            assert_eq!(
                repair_cjk_mojibake(&mojibake),
                expected,
                "修复结果不符：word={word}, mojibake={mojibake}"
            );
            assert!(word.starts_with(expected));
        }
    }

    #[test]
    fn trims_sap_style_ellipsis_suffix() {
        let mojibake = format!("{}...", simulate_gbk_mojibake("设置过滤器"));
        assert_eq!(repair_cjk_mojibake(&mojibake), "设置过滤");
    }
}
