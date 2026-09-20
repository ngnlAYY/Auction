//! 首次部署输入层校验与 DTO 转换；服务端仍独立校验完整配置和日志过滤器语法。

use std::{net::IpAddr, str::FromStr};

use auction_contracts::{SetupDatabaseRequest, SetupRequest};
use url::{Host, Url};

use super::draft::{SetupDraft, SetupField};

/// 只保存字段身份和翻译键，不在错误中带入凭据或固定当前语言。
#[derive(Clone, PartialEq)]
pub(super) struct SetupIssue {
    pub(super) field: SetupField,
    pub(super) key: &'static str,
}

/// 两种提交动作复用同一入口；任何错误都禁止构造候选，数字解析不能静默沿用旧值。
pub(super) fn validate(draft: &SetupDraft) -> Result<SetupRequest, Vec<SetupIssue>> {
    let issues: Vec<_> = SetupField::all()
        .filter_map(|field| issue(draft, field).map(|key| SetupIssue { field, key }))
        .collect();
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(SetupRequest {
        database: SetupDatabaseRequest {
            host: draft.value(SetupField::DatabaseHost).to_owned(),
            port: number(draft, SetupField::DatabasePort)?,
            username: draft.value(SetupField::DatabaseUsername).to_owned(),
            password: draft.value(SetupField::DatabasePassword).to_owned(),
            name: draft.value(SetupField::DatabaseName).to_owned(),
        },
        admin_actor: draft.value(SetupField::AdminActor).to_owned(),
        admin_secret: draft.value(SetupField::AdminSecret).to_owned(),
        public_origin: draft.value(SetupField::PublicOrigin).to_owned(),
        admin_origin: draft.value(SetupField::AdminOrigin).to_owned(),
        metrics_enabled: draft.metrics_enabled,
        schedule_enabled: draft.schedule_enabled,
        start_price: number(draft, SetupField::StartPrice)?,
        log_filter: draft.value(SetupField::LogFilter).to_owned(),
        worker_concurrency: number(draft, SetupField::WorkerConcurrency)?,
        worker_poll_interval_seconds: number(draft, SetupField::WorkerPoll)?,
        realtime_poll_interval_millis: number(draft, SetupField::RealtimePoll)?,
        confirm_initialize: false,
    })
}

fn issue(draft: &SetupDraft, field: SetupField) -> Option<&'static str> {
    let value = draft.value(field);
    if value.is_empty() {
        return Some("admin-field-required");
    }
    if field.maximum_bytes().is_some_and(|maximum| value.len() > maximum) {
        return Some("setup-field-invalid");
    }
    if let Some((minimum, maximum, step)) = field.number_bounds() {
        return (!value.bytes().all(|byte| byte.is_ascii_digit())
            || !value
                .parse::<u64>()
                .is_ok_and(|number| (minimum..=maximum).contains(&number) && number % step == 0))
        .then_some("setup-field-invalid");
    }
    let valid = match field {
        SetupField::DatabaseHost => database_text(value) && valid_host(value),
        SetupField::DatabaseUsername | SetupField::DatabasePassword => database_text(value),
        SetupField::DatabaseName => database_text(value) && !matches!(value, "." | ".."),
        SetupField::AdminActor => !value.contains([':', '\r', '\n', '\0']),
        SetupField::AdminSecret => value.len() >= 16 && !value.contains(['\r', '\n', '\0']),
        SetupField::AdminConfirmation => {
            return (value != draft.value(SetupField::AdminSecret))
                .then_some("setup-password-mismatch");
        }
        SetupField::PublicOrigin | SetupField::AdminOrigin => {
            if draft.value(SetupField::PublicOrigin) == draft.value(SetupField::AdminOrigin) {
                return Some("setup-origins-conflict");
            }
            valid_origin(value)
        }
        // 不在浏览器复制 EnvFilter 的语法解析器；本地只校验无歧义的文本边界。
        SetupField::LogFilter => !value.contains(['\r', '\n', '\0']),
        _ => true,
    };
    (!valid).then_some("setup-field-invalid")
}

fn database_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn valid_host(value: &str) -> bool {
    if value != value.trim() || value.contains('%') {
        return false;
    }
    Url::parse("postgresql://localhost").is_ok_and(|mut url| {
        if let Ok(address) = value.parse::<IpAddr>() {
            url.set_ip_host(address).is_ok()
        } else {
            Host::parse(value).is_ok_and(|host| url.set_host(Some(&host.to_string())).is_ok())
        }
    })
}

/// 保持首次部署 owner 的精确 Origin 规则；不自动去掉路径或改写输入的站点。
fn valid_origin(value: &str) -> bool {
    Url::parse(value).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https")
            && (parsed.scheme() == "https"
                || matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")))
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.origin().ascii_serialization() == value
    })
}

fn number<T: FromStr>(draft: &SetupDraft, field: SetupField) -> Result<T, Vec<SetupIssue>> {
    draft.value(field).parse().map_err(|_| vec![SetupIssue { field, key: "setup-field-invalid" }])
}
