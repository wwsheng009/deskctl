//! 通用"标签 → 数值"配对：把 OCR 元素按行分组，再按几何关系左右/上下配对。
//!
//! 适用于老式表单/监控界面（SAP、ERP、终端）把"标签列 + 数值列"读成结构化数据。

/// 参与配对的 OCR 元素（图像坐标系）。
#[derive(Debug, Clone)]
pub struct KvElement {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
}

impl KvElement {
    fn center_y(&self) -> i32 {
        self.y + self.height as i32 / 2
    }

    fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }
}

/// 一条配对结果。
#[derive(Debug, Clone)]
pub struct KvPair {
    pub label: KvElement,
    pub value: KvElement,
    pub row: usize,
    /// 同一行/列存在多个同样近的候选取值时为 true（调用方可复核）。
    pub ambiguous: bool,
}

/// 配对参数。
#[derive(Debug, Clone)]
pub struct KvOptions {
    /// 标签与右侧取值允许的最大水平间距（像素）。
    pub value_gap: i32,
    /// 行分组容差（像素）；0 表示按中位字高自动计算。
    pub row_tolerance: i32,
    /// 低于该置信度的元素不参与配对。
    pub min_confidence: f32,
    /// 最多返回的配对数。
    pub max_pairs: usize,
}

impl Default for KvOptions {
    fn default() -> Self {
        Self {
            value_gap: 600,
            row_tolerance: 0,
            min_confidence: 0.3,
            max_pairs: 500,
        }
    }
}

/// 对元素做行分组 + 配对，返回（配对列表，未配对元素）。
pub fn pair_key_values(
    elements: &[KvElement],
    options: &KvOptions,
) -> (Vec<KvPair>, Vec<KvElement>) {
    let mut items: Vec<KvElement> = elements
        .iter()
        .filter(|element| {
            !element.text.trim().is_empty()
                && element.width > 0
                && element.height > 0
                && element.confidence >= options.min_confidence
        })
        .cloned()
        .collect();
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }
    items.sort_by_key(|element| (element.center_y(), element.x));

    let tolerance = if options.row_tolerance > 0 {
        options.row_tolerance
    } else {
        let mut heights: Vec<u32> = items.iter().map(|element| element.height).collect();
        heights.sort_unstable();
        let median = heights[heights.len() / 2] as i32;
        (median / 2).clamp(4, 40)
    };

    // 行分组：保存每行（中心 y + 元素下标），保持自上而下的顺序。
    let mut rows: Vec<(i32, Vec<usize>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let center = item.center_y();
        match rows.last_mut() {
            Some((row_center, row)) if (center - *row_center).abs() <= tolerance => {
                row.push(index);
            }
            _ => rows.push((center, vec![index])),
        }
    }

    let mut used = vec![false; items.len()];
    let mut pairs: Vec<KvPair> = Vec::new();
    for (row_index, (_, mut row)) in rows.into_iter().enumerate() {
        if pairs.len() >= options.max_pairs {
            break;
        }
        row.sort_by_key(|index| items[*index].x);
        for position in 0..row.len() {
            if pairs.len() >= options.max_pairs {
                break;
            }
            let label_index = row[position];
            if used[label_index] {
                continue;
            }
            let label = items[label_index].clone();
            // 1) 同行右侧：最近且 gap 不超过上限者。
            let mut best: Option<(usize, i32)> = None;
            let mut candidates = 0usize;
            for other_index in row.iter().skip(position + 1) {
                let other = &items[*other_index];
                if used[*other_index] {
                    continue;
                }
                if other.x < label.x + (label.width as i32) / 2 {
                    continue;
                }
                let gap = other.x - label.right();
                if gap < 0 {
                    continue;
                }
                if gap > options.value_gap {
                    break;
                }
                candidates += 1;
                if best.is_none_or(|(_, best_gap)| gap < best_gap) {
                    best = Some((*other_index, gap));
                }
            }
            // 2) 下方：垂直相邻、水平重叠且 gap 不超过上限者（跨行查找）。
            if best.is_none() {
                let mut best_below: Option<(usize, i32)> = None;
                for (other_index, other) in items.iter().enumerate() {
                    if other_index == label_index || used[other_index] {
                        continue;
                    }
                    let overlap = overlap_x(&label, other);
                    if overlap <= 0 {
                        continue;
                    }
                    let gap = other.y - label.bottom();
                    if gap < 0 || gap > options.value_gap {
                        continue;
                    }
                    candidates += 1;
                    if best_below.is_none_or(|(_, best_gap)| gap < best_gap) {
                        best_below = Some((other_index, gap));
                    }
                }
                best = best_below;
            }
            if let Some((value_index, _)) = best {
                used[label_index] = true;
                used[value_index] = true;
                pairs.push(KvPair {
                    label,
                    value: items[value_index].clone(),
                    row: row_index,
                    ambiguous: candidates > 1,
                });
            }
        }
    }

    let unpaired: Vec<KvElement> = items
        .iter()
        .enumerate()
        .filter(|(index, _)| !used[*index])
        .map(|(_, element)| element.clone())
        .collect();
    (pairs, unpaired)
}

fn overlap_x(left: &KvElement, right: &KvElement) -> i32 {
    let start = left.x.max(right.x);
    let end = left.right().min(right.right());
    end - start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(text: &str, x: i32, y: i32, width: u32, height: u32) -> KvElement {
        KvElement {
            text: text.to_string(),
            x,
            y,
            width,
            height,
            confidence: 0.9,
        }
    }

    #[test]
    fn pairs_labels_with_right_values_in_rows() {
        // SAP ST06 风格：标签列 x≈761，数值列 x≈1359。
        let elements = vec![
            element("物理内存", 761, 915, 81, 11),
            element("64.414MB", 1359, 915, 81, 11),
            element("主机名", 761, 394, 47, 13),
            element("sap1809demo", 1302, 396, 104, 14),
            element("页换入", 761, 1121, 45, 14),
            element("0KB/s", 1402, 1123, 50, 12),
        ];
        let (pairs, unpaired) = pair_key_values(&elements, &KvOptions::default());
        assert_eq!(pairs.len(), 3, "unpaired={unpaired:?}");
        assert_eq!(pairs[0].label.text, "主机名");
        assert_eq!(pairs[0].value.text, "sap1809demo");
        assert_eq!(pairs[1].label.text, "物理内存");
        assert_eq!(pairs[1].value.text, "64.414MB");
        assert_eq!(pairs[2].label.text, "页换入");
        assert_eq!(pairs[2].value.text, "0KB/s");
        assert!(unpaired.is_empty());
    }

    #[test]
    fn falls_back_to_value_below_the_label() {
        let elements = vec![
            element("可用内存", 100, 100, 80, 12),
            element("4,105 MB", 110, 130, 70, 12),
        ];
        let (pairs, _) = pair_key_values(&elements, &KvOptions::default());
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].value.text, "4,105 MB");
    }

    #[test]
    fn reports_ambiguous_rows_and_filters_noise() {
        let mut elements = vec![
            element("指标", 10, 10, 40, 10),
            element("值", 100, 10, 20, 10),
            element("单位", 140, 10, 30, 10),
            element("", 300, 10, 30, 10),
        ];
        elements.push(KvElement {
            text: "低置信".to_string(),
            x: 10,
            y: 40,
            width: 40,
            height: 10,
            confidence: 0.1,
        });
        let (pairs, unpaired) = pair_key_values(&elements, &KvOptions::default());
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].label.text, "指标");
        assert!(pairs[0].ambiguous, "指标右侧有多个候选");
        assert!(
            !unpaired.iter().any(|element| element.text == "低置信"),
            "低于置信度阈值的元素既不参与配对也不出现在 unpaired"
        );
        let _ = elements.len();
    }

    #[test]
    fn respects_gap_limit() {
        let elements = vec![
            element("标签", 10, 10, 40, 10),
            element("值", 900, 10, 20, 10),
        ];
        let (pairs, unpaired) = pair_key_values(&elements, &KvOptions::default());
        assert!(pairs.is_empty());
        assert_eq!(unpaired.len(), 2);
    }
}
