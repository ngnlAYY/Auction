//! 首次部署表单：只在独立初始化进程中由 `/setup` 提供。

use auction_contracts::{SetupDatabaseRequest, SetupRequest};
use auction_web_shared::{LanguagePicker, legacy_t as t, use_app_boot_ready};
use dioxus::prelude::*;

use crate::api::submit_first_run;

/// 展示数据库、管理员、基础及进阶参数，完成后等待部署器启动正式进程。
pub(super) fn page() -> Element {
    use_app_boot_ready(true);
    let mut token = use_signal(String::new);
    let mut form = use_signal(|| SetupRequest {
        database: SetupDatabaseRequest {
            host: "127.0.0.1".to_owned(),
            port: 5432,
            username: String::new(),
            password: String::new(),
            name: "auction".to_owned(),
        },
        admin_actor: String::new(),
        admin_secret: String::new(),
        public_origin: String::new(),
        admin_origin: String::new(),
        metrics_enabled: true,
        schedule_enabled: false,
        start_price: 1000,
        log_filter: "info,auction=debug".to_owned(),
        worker_concurrency: 4,
        worker_poll_interval_seconds: 15,
        realtime_poll_interval_millis: 250,
    });
    let mut busy = use_signal(|| false);
    let mut notice = use_signal(|| None::<(bool, String)>);
    let mut completed = use_signal(|| false);

    rsx! {
        main { class: "admin-setup", aria_labelledby: "admin-setup-title",
            div { class: "admin-setup__card",
                LanguagePicker { label: t!("language-label"), default_locale: "zh-CN" }
                h1 { id: "admin-setup-title", {t!("setup-title")} }
                p { class: "admin-panel__note", {t!("setup-intro")} }
                form {
                    class: "admin-setup__form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() || completed() { return; }
                        busy.set(true);
                        notice.set(None);
                        let submitted_token = token();
                        let submitted_form = form();
                        spawn(async move {
                            match submit_first_run(&submitted_token, &submitted_form, true).await {
                                Ok(()) => {
                                    completed.set(true);
                                    form.write().admin_secret.clear();
                                    form.write().database.password.clear();
                                    token.set(String::new());
                                    notice.set(Some((true, "setup-complete".to_owned())));
                                }
                                Err(code) => notice.set(Some((false, error_key(&code).to_owned()))),
                            }
                            busy.set(false);
                        });
                    },
                    fieldset { disabled: busy() || completed(),
                        legend { {t!("setup-access")} }
                        label { for: "setup-token", {t!("setup-token")} }
                        input { id: "setup-token", r#type: "password", autocomplete: "off", required: true,
                            value: "{token}", oninput: move |event| token.set(event.value()) }
                    }
                    fieldset { disabled: busy() || completed(),
                        legend { {t!("setup-database")} }
                        label { for: "setup-database-host", {t!("setup-database-host")} }
                        input { id: "setup-database-host", r#type: "text", autocomplete: "off", required: true,
                            value: "{form().database.host}", oninput: move |event| form.write().database.host = event.value() }
                        label { for: "setup-database-port", {t!("setup-database-port")} }
                        input { id: "setup-database-port", r#type: "number", min: "1", max: "65535", required: true,
                            value: "{form().database.port}", oninput: move |event| {
                                form.write().database.port = event.value().parse().unwrap_or(0);
                            } }
                        label { for: "setup-database-username", {t!("setup-database-username")} }
                        input { id: "setup-database-username", r#type: "text", autocomplete: "off", required: true,
                            value: "{form().database.username}", oninput: move |event| form.write().database.username = event.value() }
                        label { for: "setup-database-password", {t!("setup-database-password")} }
                        input { id: "setup-database-password", r#type: "password", autocomplete: "new-password", required: true,
                            value: "{form().database.password}", oninput: move |event| form.write().database.password = event.value() }
                        label { for: "setup-database-name", {t!("setup-database-name")} }
                        input { id: "setup-database-name", r#type: "text", autocomplete: "off", required: true,
                            value: "{form().database.name}", oninput: move |event| form.write().database.name = event.value() }
                        p { class: "admin-panel__note", {t!("setup-database-help")} }
                    }
                    fieldset { disabled: busy() || completed(),
                        legend { {t!("setup-admin")} }
                        label { for: "setup-admin-actor", {t!("setup-admin-actor")} }
                        input { id: "setup-admin-actor", r#type: "text", autocomplete: "username", required: true,
                            value: "{form().admin_actor}", oninput: move |event| form.write().admin_actor = event.value() }
                        label { for: "setup-admin-secret", {t!("setup-admin-secret")} }
                        input { id: "setup-admin-secret", r#type: "password", autocomplete: "new-password", required: true,
                            value: "{form().admin_secret}", oninput: move |event| form.write().admin_secret = event.value() }
                    }
                    fieldset { disabled: busy() || completed(),
                        legend { {t!("setup-basic")} }
                        label { for: "setup-public-origin", {t!("setup-public-origin")} }
                        input { id: "setup-public-origin", r#type: "url", required: true,
                            placeholder: "https://auction.example.com",
                            value: "{form().public_origin}", oninput: move |event| form.write().public_origin = event.value() }
                        label { for: "setup-admin-origin", {t!("setup-admin-origin")} }
                        input { id: "setup-admin-origin", r#type: "url", required: true,
                            placeholder: "https://admin.example.com",
                            value: "{form().admin_origin}", oninput: move |event| form.write().admin_origin = event.value() }
                        label { for: "setup-start-price", {t!("setup-start-price")} }
                        input { id: "setup-start-price", r#type: "number", min: "1000", max: "1000000000", step: "1000", required: true,
                            value: "{form().start_price}", oninput: move |event| {
                                if let Ok(value) = event.value().parse() { form.write().start_price = value; }
                            } }
                        label { class: "admin-setup__check",
                            input { r#type: "checkbox", checked: form().metrics_enabled,
                                onchange: move |event| form.write().metrics_enabled = event.checked() }
                            {t!("setup-metrics")}
                        }
                        label { class: "admin-setup__check",
                            input { r#type: "checkbox", checked: form().schedule_enabled,
                                onchange: move |event| form.write().schedule_enabled = event.checked() }
                            {t!("setup-schedule")}
                        }
                    }
                    fieldset { disabled: busy() || completed(),
                        legend { {t!("setup-advanced")} }
                        label { for: "setup-log-filter", {t!("setup-log-filter")} }
                        input { id: "setup-log-filter", r#type: "text", required: true,
                            value: "{form().log_filter}", oninput: move |event| form.write().log_filter = event.value() }
                        label { for: "setup-worker-concurrency", {t!("setup-worker-concurrency")} }
                        input { id: "setup-worker-concurrency", r#type: "number", min: "1", max: "8", required: true,
                            value: "{form().worker_concurrency}", oninput: move |event| {
                                if let Ok(value) = event.value().parse() { form.write().worker_concurrency = value; }
                            } }
                        label { for: "setup-worker-poll", {t!("setup-worker-poll")} }
                        input { id: "setup-worker-poll", r#type: "number", min: "11", max: "300", required: true,
                            value: "{form().worker_poll_interval_seconds}", oninput: move |event| {
                                if let Ok(value) = event.value().parse() { form.write().worker_poll_interval_seconds = value; }
                            } }
                        label { for: "setup-realtime-poll", {t!("setup-realtime-poll")} }
                        input { id: "setup-realtime-poll", r#type: "number", min: "100", max: "5000", required: true,
                            value: "{form().realtime_poll_interval_millis}", oninput: move |event| {
                                if let Ok(value) = event.value().parse() { form.write().realtime_poll_interval_millis = value; }
                            } }
                    }
                    div { class: "admin-setup__actions",
                        button { r#type: "button", disabled: busy() || completed(),
                            onclick: move |_| {
                                if busy() || completed() { return; }
                                busy.set(true);
                                notice.set(None);
                                let submitted_token = token();
                                let submitted_form = form();
                                spawn(async move {
                                    let result = submit_first_run(&submitted_token, &submitted_form, false).await;
                                    notice.set(Some(match result {
                                        Ok(()) => (true, "setup-valid".to_owned()),
                                        Err(code) => (false, error_key(&code).to_owned()),
                                    }));
                                    busy.set(false);
                                });
                            },
                            {t!("setup-validate")}
                        }
                        button { class: "admin-btn-primary", r#type: "submit", disabled: busy() || completed(),
                            {t!("setup-save")}
                        }
                    }
                }
                if let Some((success, key)) = notice() {
                    p { class: if success { "action-feedback" } else { "action-feedback action-feedback--error" },
                        role: "status", {t!(&key)} }
                }
            }
        }
    }
}

fn error_key(code: &str) -> &'static str {
    match code {
        "invalid_input" => "setup-error-input",
        "database_unavailable" => "setup-error-database",
        "configuration_invalid" => "setup-error-config",
        "setup_locked" | "setup_completed" => "setup-error-locked",
        "setup_busy" => "setup-error-busy",
        "setup_auth" => "setup-error-token",
        "setup_io" => "setup-error-io",
        "setup_outcome_unknown" => "setup-error-outcome-unknown",
        "network" => "setup-error-network",
        _ => "setup-error-unknown",
    }
}
