//! Admin runtime 配置页面。
//!
//! 页面只编辑服务端下发的非敏感 JSON 投影；更新结果明确显示 restart-required，避免把
//! 文件原子替换误报成当前进程已经热加载。

use dioxus::prelude::*;
use dioxus_i18n::t;
use serde_json::Value;

use crate::api::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};

use super::AdminPanel;

pub(super) fn page() -> Element {
    let mut editor = use_signal(String::new);
    let mut fingerprint = use_signal(String::new);
    let mut notice = use_signal(|| None::<bool>);
    let mut refresh = use_signal(|| 0_u64);
    let config = use_resource(move || {
        let _ = refresh();
        fetch_admin_runtime_config()
    });

    match &*config.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-config-title"), detail: t!("admin-runtime-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminPanel { title: t!("page-config-title"), detail: t!("admin-runtime-error") }
        },
        Some(Ok(config)) => {
            let editor_value =
                if editor().is_empty() { format_document(&config.document) } else { editor() };
            let current_fingerprint =
                if fingerprint().is_empty() { config.fingerprint.clone() } else { fingerprint() };
            let validate_editor_value = editor_value.clone();
            let update_editor_value = editor_value.clone();
            let update_fingerprint = current_fingerprint.clone();
            rsx! {
                section { class: "admin-panel", aria_labelledby: "admin-runtime-title",
                    h1 { id: "admin-runtime-title", {t!("page-config-title")} }
                    p { class: "admin-panel__note", {t!("admin-runtime-note")} }
                    p { class: "admin-panel__note",
                        {t!("admin-runtime-fingerprint", fingerprint: current_fingerprint.clone())}
                    }
                    label { class: "admin-runtime-config__label",
                        {t!("admin-runtime-editor")}
                        textarea {
                            class: "admin-runtime-config__editor",
                            value: editor_value.clone(),
                            oninput: move |event| editor.set(event.value()),
                        }
                    }
                    div { class: "admin-panel__actions",
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                let text = validate_editor_value.clone();
                                notice.set(None);
                                let document = serde_json::from_str::<Value>(&text);
                                let Some(document) = document.ok() else {
                                    notice.set(Some(false));
                                    return;
                                };
                                spawn(async move {
                                    match validate_admin_runtime_config(document).await {
                                        Ok(result) => {
                                            editor.set(format_document(&result.document));
                                            fingerprint.set(result.fingerprint);
                                            notice.set(Some(true));
                                        }
                                        Err(_) => notice.set(Some(false)),
                                    }
                                });
                            },
                            {t!("admin-runtime-validate")}
                        }
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                let text = update_editor_value.clone();
                                let expected = update_fingerprint.clone();
                                notice.set(None);
                                let document = serde_json::from_str::<Value>(&text);
                                let Some(document) = document.ok() else {
                                    notice.set(Some(false));
                                    return;
                                };
                                spawn(async move {
                                    match update_admin_runtime_config(expected, document).await {
                                        Ok(result) => {
                                            editor.set(format_document(&result.document));
                                            fingerprint.set(result.fingerprint);
                                            notice.set(Some(true));
                                            refresh.set(refresh() + 1);
                                        }
                                        Err(_) => notice.set(Some(false)),
                                    }
                                });
                            },
                            {t!("admin-runtime-update")}
                        }
                    }
                    if config.restart_required {
                        p { class: "admin-panel__note", role: "status", {t!("admin-runtime-restart-required")} }
                    }
                    if let Some(successful) = notice() {
                        p { class: "admin-panel__note", role: "status",
                            if successful { {t!("admin-runtime-action-success")} }
                            else { {t!("admin-runtime-action-error")} }
                        }
                    }
                }
            }
        }
    }
}

fn format_document(document: &Value) -> String {
    serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned())
}
