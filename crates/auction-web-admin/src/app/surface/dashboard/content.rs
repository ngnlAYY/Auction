//! 旧版 Admin Dashboard 的指标顺序与展示闭集。
//! 快捷入口、摘要卡与各指标分组按旧版 DOM 顺序装配；状态判断集中在 [`derived`]，
//! 每个分组的渲染落在各自子模块，避免单文件承载全部面板。

use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

mod alerting;
mod audit;
mod credentials;
mod deliveries;
mod derived;
mod fulfillment;
mod outbox;
mod reconciliation;
mod recovery;
mod shared;
mod summary;
mod tracing;

use derived::DashboardDerived;

#[component]
pub(super) fn DashboardContent(snapshot: AdminDashboardSnapshot, children: Element) -> Element {
    // 状态只推导一次，各面板读取同一份结果以保证同快照下状态一致。
    let derived = DashboardDerived::new(&snapshot);

    rsx! {
        Fragment {
            {summary::summary_section(&snapshot, &derived)}

            {children}

            // 每个分组是完整阅读单元；宽屏分栏，窄屏按原有 DOM 顺序展开。
            div { class: "admin-dashboard-panels",
                {audit::audit_panel(&snapshot, &derived)}
                {tracing::tracing_panel(&snapshot)}
                {outbox::outbox_panel(&snapshot, &derived)}
                {credentials::credentials_panel(&snapshot)}
                {deliveries::deliveries_panel(&snapshot)}
                {fulfillment::fulfillment_panel(&snapshot, &derived)}
                {recovery::recovery_panel(&snapshot, &derived)}
                {reconciliation::reconciliation_panel(&snapshot, &derived)}
                {alerting::alerting_panel(&snapshot)}
            }
        }
    }
}
