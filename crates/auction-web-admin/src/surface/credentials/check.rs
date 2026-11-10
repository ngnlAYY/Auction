//! 手动 Cookie 校验按钮与瞬时结果；不在浏览器读取或保存 Cookie。

use auction_web_shared::{Time, legacy_t as t};
use dioxus::prelude::*;

use super::credential_world_label;
use crate::{
    api::{AdminCredential, AdminCredentialCheck, check_admin_credential},
    surface::{AdminActionFeedback, run_admin_feedback_action},
};

/// 当前凭据版本的手动校验入口与安全结果。
#[component]
pub fn CredentialCheckAction(
    credential: AdminCredential,
    busy: Signal<bool>,
    blocked: bool,
) -> Element {
    let mut checked = use_signal(|| None::<AdminCredentialCheck>);
    let mut checking = use_signal(|| false);
    let feedback = use_signal(|| None::<AdminActionFeedback>);
    let result_id = format!("credential-check-{}", credential.id);
    // 父页刷新/其他管理员修改版本后，旧成功结果立即失效，不混入新凭据投影。
    let visible = checked().filter(|result| {
        result.credential_id == credential.id && result.state_version == credential.state_version
    });
    let failed = feedback().filter(|feedback| !feedback.is_success());
    let described_by = (visible.is_some() || failed.is_some()).then(|| result_id.clone());
    rsx! {
        button {
            r#type: "button",
            disabled: busy() || blocked,
            aria_busy: checking(),
            aria_describedby: described_by,
            onclick: move |_| {
                if busy() || blocked { return; }
                checked.set(None);
                checking.set(true);
                let credential = credential.clone();
                run_admin_feedback_action(busy, feedback, async move {
                    let result = check_admin_credential(&credential).await;
                    checking.set(false);
                    result
                }, move |result| checked.set(Some(result)));
            },
            if checking() { {t!("admin-credentials-checking")} } else { {t!("admin-credentials-action-check")} }
        }
        if let Some(result) = visible {
            span { id: result_id.clone(), class: "admin-credentials__feedback", role: "status",
                for check in result.checks {
                    span {
                        class: match check.status.as_str() {
                            "valid" => "admin-credentials__feedback--success",
                            "invalid" => "admin-credentials__feedback--error",
                            _ => "",
                        },
                        {t!("admin-credentials-check-result", world: credential_world_label(&check.world), status: check_status_label(&check.status))}
                        " "
                    }
                }
                {t!("admin-credentials-check-time")}
                Time { value: result.checked_at }
            }
        } else if let Some(failed) = failed {
            span { id: result_id.clone(), class: "admin-credentials__feedback admin-credentials__feedback--error", role: "alert",
                {failed.message(t!("admin-credentials-check-failed"))}
            }
        }
    }
}

fn check_status_label(status: &str) -> String {
    match status {
        "valid" => t!("admin-credentials-check-valid"),
        "invalid" => t!("admin-credentials-check-invalid"),
        _ => t!("admin-credentials-check-inconclusive"),
    }
}
