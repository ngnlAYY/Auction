//! 邮件入站运行观测；明确区分已完成游标、当前目标和最近失败，不提供跳过或重发动作。

use auction_contracts::AdminMailPollingDiagnosticsResponse;
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::super::format_admin_time;

#[component]
pub(super) fn MailPollingProgress(
    diagnostics: Option<AdminMailPollingDiagnosticsResponse>,
) -> Element {
    let Some(diagnostics) = diagnostics else {
        return rsx! { p { {t!("admin-mail-progress-not-loaded")} } };
    };
    let cursor = match diagnostics.cursor_status.as_str() {
        "available" => {
            rsx! { code { {diagnostics.completed_external_mail_id.clone().unwrap_or_default()} } }
        }
        "empty" => rsx! { {t!("admin-mail-progress-cursor-empty")} },
        _ => rsx! { {t!("admin-mail-progress-cursor-hidden")} },
    };
    rsx! {
        div { class: "admin-mail-progress",
            h4 { {t!("admin-mail-progress-title")} }
            p { class: "admin-mail-progress__hint", {t!("admin-mail-progress-note")} }
            dl {
                div { dt { {t!("admin-mail-progress-cursor")} } dd { {cursor} } }
                div { dt { {t!("admin-mail-progress-observed")} } dd { {format_admin_time(&diagnostics.observed_at)} } }
            }
            if diagnostics.worker_condition != "available" {
                p { {t!("admin-mail-progress-unavailable")} }
            } else if let Some(progress) = &diagnostics.progress {
                dl {
                    div { dt { {t!("admin-mail-progress-stage")} } dd { {stage_label(&progress.stage)} } }
                    div {
                        dt { {t!("admin-mail-progress-current")} }
                        dd {
                            if let Some(mail_id) = &progress.external_mail_id { code { "{mail_id}" } }
                            else { {t!("admin-mail-progress-no-mid")} }
                        }
                    }
                    div { dt { {t!("admin-mail-progress-updated")} } dd { {format_admin_time(&progress.updated_at)} } }
                    div { dt { {t!("admin-mail-progress-worker-observed")} } dd { {format_admin_time(diagnostics.worker_observed_at.as_deref().unwrap_or_default())} } }
                }
                if let Some(failure) = &progress.last_failure {
                    dl {
                        div {
                            dt { {t!("admin-mail-progress-failed-mid")} }
                            dd {
                                if let Some(mail_id) = &failure.external_mail_id { code { "{mail_id}" } }
                                else { {t!("admin-mail-progress-no-mid")} }
                            }
                        }
                        div { dt { {t!("admin-mail-progress-failed-stage")} } dd { {stage_label(&failure.stage)} } }
                        div { dt { {t!("admin-mail-progress-failed-at")} } dd { {format_admin_time(&failure.occurred_at)} } }
                    }
                } else { p { {t!("admin-mail-progress-no-failure")} } }
            } else { p { {t!("admin-mail-progress-not-observed")} } }
        }
    }
}

fn stage_label(stage: &str) -> String {
    match stage {
        "fetching" => t!("admin-mail-progress-fetching"),
        "reading_detail" => t!("admin-mail-progress-reading-detail"),
        "processing" => t!("admin-mail-progress-processing"),
        "committing" => t!("admin-mail-progress-committing"),
        "idle" => t!("admin-mail-progress-idle"),
        "failed" => t!("admin-mail-progress-failed"),
        "canceled" => t!("admin-mail-progress-canceled"),
        _ => t!("admin-mail-progress-unavailable"),
    }
}
