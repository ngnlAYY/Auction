//! 表格列比例的渲染原语；字段顺序与具体比例由各业务表格拥有。

use dioxus::prelude::*;

/// 按调用方给定的列权重生成 colgroup，不推断业务字段，也不套用全站等分比例。
///
/// 业务表通常传入总和为 100 的百分比；Markdown 可以传入由内容长度估算的正权重。
/// 使用同一总和归一化，保证列布局始终受 100% 容器约束。
#[component]
pub fn TableColumns(layout: &'static str, widths: Vec<u16>) -> Element {
    let total = widths.iter().map(|width| u32::from(*width)).sum::<u32>().max(1);
    rsx! {
        colgroup { "data-table-layout": layout,
            for (index, width) in widths.into_iter().enumerate() {
                col {
                    key: "{index}",
                    style: format!("width: {:.6}%", f64::from(width) * 100.0 / f64::from(total)),
                }
            }
        }
    }
}
