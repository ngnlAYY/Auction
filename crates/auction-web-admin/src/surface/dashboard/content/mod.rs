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
mod external_accounts;
mod fulfillment;
mod outbox;
mod reconciliation;
mod recovery;
mod summary;
mod tracing;
mod widgets;

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
                {external_accounts::external_accounts_panel(&snapshot)}
                {deliveries::deliveries_panel(&snapshot)}
                {fulfillment::fulfillment_panel(&snapshot, &derived)}
                {recovery::recovery_panel(&snapshot, &derived)}
                {reconciliation::reconciliation_panel(&snapshot, &derived)}
                {alerting::alerting_panel(&snapshot)}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::build_bundle;
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use serde_json::{Value, json};

    use super::{audit, credentials, derived::DashboardDerived, outbox, summary};
    use crate::api::AdminDashboardSnapshot;
    use crate::surface::ADMIN_LOCALE_CATALOG;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Panel {
        Summary,
        Audit,
        Credentials,
        Outbox,
    }

    #[component]
    fn PanelHarness(panel: Panel, snapshot: AdminDashboardSnapshot) -> Element {
        let catalog = &ADMIN_LOCALE_CATALOG;
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &catalog.default_locale,
                catalog.default_ftl,
                &catalog.default_locale,
                None,
            )
        })
        .unwrap();
        match panel {
            Panel::Summary => {
                summary::summary_metrics(&snapshot, &DashboardDerived::new(&snapshot))
            }
            Panel::Audit => audit::audit_panel(&snapshot, &DashboardDerived::new(&snapshot)),
            Panel::Credentials => credentials::credentials_panel(&snapshot),
            Panel::Outbox => outbox::outbox_panel(&snapshot, &DashboardDerived::new(&snapshot)),
        }
    }

    fn panel_text(panel: Panel, update: impl FnOnce(&mut Value)) -> Vec<String> {
        let snapshot = AdminDashboardSnapshot::test_fixture(update);
        let mut dom =
            VirtualDom::new_with_props(PanelHarness, PanelHarnessProps { panel, snapshot });
        dom.rebuild_to_vec()
            .edits
            .into_iter()
            .filter_map(|edit| match edit {
                Mutation::CreateTextNode { value, .. } => Some(value),
                _ => None,
            })
            .collect()
    }

    const OBSERVED_AT: &str = "2026-01-01T00:01:00Z";

    fn realtime(oldest_at: Option<&str>, lag: i64) -> Value {
        json!({
            "status": "degraded",
            "observed_at": OBSERVED_AT,
            "data": {
                "observed_at": OBSERVED_AT,
                "total_lag": lag,
                "lags": [{
                    "stream_key": "auction:main",
                    "consumer_key": "diagnostics",
                    "offset": 0,
                    "latest_sequence": lag,
                    "lag": lag,
                    "last_advanced_at": null
                }],
                "failure": {
                    "count": i64::from(oldest_at.is_some()),
                    "oldest_at": oldest_at,
                    "latest_code": oldest_at.map_or("", |_| "consumer_lag")
                }
            }
        })
    }

    #[test]
    fn missing_realtime_and_alerting_counts_are_distinct_from_measured_zeroes() {
        let missing =
            panel_text(Panel::Summary, |payload| payload["alerting"] = payload["mail"].clone());
        assert_eq!(
            missing.iter().filter(|text| text.as_str() == "暂无数据。").count(),
            2,
            "{missing:?}"
        );
        let measured = panel_text(Panel::Summary, |payload| {
            payload["realtime"] = realtime(None, 0);
            payload["alerting"]["data"]["active_signal_count"] = json!(0);
        });
        assert_eq!(measured.iter().filter(|text| text.as_str() == "0").count(), 2);
    }

    #[test]
    fn admin_heartbeat_is_part_of_audit_coverage() {
        let incomplete = panel_text(Panel::Audit, |payload| {
            payload["health"]["status"] = json!("degraded");
            payload["health"]["readiness"] = json!(false);
            payload["health"]["missing_services"] = json!(["admin"]);
        });
        assert!(incomplete.contains(&"状态未知".to_owned()), "{incomplete:?}");
        assert!(incomplete.iter().any(|text| text.contains("Admin API 不可用")));
        let complete = panel_text(Panel::Audit, |_| {});
        assert!(complete.contains(&"正常".to_owned()));
        assert!(complete.contains(&"0".to_owned()));
    }

    #[test]
    fn cookie_database_count_does_not_imply_a_worker_restart_has_happened() {
        for (available, database_status) in [(0, "无活跃 Cookie"), (1, "已入库")] {
            for (restart_required, worker_status) in [
                (Some(true), "Worker 待重启"),
                (Some(false), "Worker 无需重启"),
                (None, "无法确认 Worker 是否需要重启"),
            ] {
                let status = panel_text(Panel::Credentials, |payload| {
                    payload["credentials"] = json!({
                        "status": "degraded",
                        "observed_at": OBSERVED_AT,
                        "data": {
                            "availability": [{
                                "world": "main", "available": available, "invalid": 0,
                                "restart_required": restart_required, "updated_at": OBSERVED_AT
                            }],
                            "observed_at": OBSERVED_AT,
                            "last_admin_change_at": OBSERVED_AT
                        }
                    });
                });
                assert!(status.iter().any(|text| text == database_status), "{status:?}");
                assert!(status.iter().any(|text| text == worker_status), "{status:?}");
                assert!(status.contains(&available.to_string()));
            }
        }
    }

    #[test]
    fn outbox_age_describes_unresolved_failure_at_observation_time() {
        let backlog = panel_text(Panel::Outbox, |payload| payload["realtime"] = realtime(None, 5));
        assert!(backlog.contains(&"当前没有未解决的投递流失败。".to_owned()), "{backlog:?}");
        assert!(backlog.contains(&"5".to_owned()));
        let failure = panel_text(Panel::Outbox, |payload| {
            payload["realtime"] = realtime(Some("2026-01-01T00:00:00Z"), 5);
        });
        assert!(
            failure.contains(&"最早未解决的投递流失败距快照采集时已过 60 秒".to_owned()),
            "{failure:?}"
        );
    }
}
