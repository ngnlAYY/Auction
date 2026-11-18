//! 首次部署输入层校验与 DTO 转换；服务端仍独立校验完整配置和日志过滤器语法。

use std::{net::IpAddr, str::FromStr};

use auction_contracts::{SetupDatabaseRequest, SetupRequest, SetupScheduleRequest};
use url::{Host, Url};

use super::draft::{DatabaseTarget, SetupDraft, SetupField};
use crate::api::valid_safe_text;

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
        public_listener_port: number(draft, SetupField::PublicListenerPort)?,
        admin_listener_port: number(draft, SetupField::AdminListenerPort)?,
        metrics_enabled: draft.metrics_enabled,
        schedule_enabled: draft.schedule_enabled,
        schedule: SetupScheduleRequest {
            timezone: draft.value(SetupField::ScheduleTimezone).to_owned(),
            preview_day: draft.value(SetupField::SchedulePreviewDay).to_owned(),
            preview_time: draft.value(SetupField::SchedulePreviewTime).to_owned(),
            open_day: draft.value(SetupField::ScheduleOpenDay).to_owned(),
            open_time: draft.value(SetupField::ScheduleOpenTime).to_owned(),
            nominal_end_day: draft.value(SetupField::ScheduleEndDay).to_owned(),
            nominal_end_time: draft.value(SetupField::ScheduleEndTime).to_owned(),
        },
        start_price: number(draft, SetupField::StartPrice)?,
        log_filter: draft.value(SetupField::LogFilter).to_owned(),
        worker_concurrency: number(draft, SetupField::WorkerConcurrency)?,
        worker_poll_interval_seconds: number(draft, SetupField::WorkerPoll)?,
        realtime_poll_interval_millis: number(draft, SetupField::RealtimePoll)?,
        confirm_initialize: false,
    })
}

/// 确认只接收可提交的目标字段，避免把无效输入呈现成可授权的目标。
pub(super) fn database_target(draft: &SetupDraft) -> Option<DatabaseTarget> {
    let [host, port, name] = SetupField::DATABASE_TARGET;
    if SetupField::DATABASE_TARGET.into_iter().any(|field| issue(draft, field).is_some()) {
        return None;
    }
    Some(DatabaseTarget {
        host: draft.value(host).to_owned(),
        port: draft.value(port).parse().ok()?,
        name: draft.value(name).to_owned(),
    })
}

pub(super) fn issue(draft: &SetupDraft, field: SetupField) -> Option<&'static str> {
    let value = draft.value(field);
    if field == SetupField::AdminActor {
        // 与首次设置配置的 128 字节边界及正式登录的冒号约束对齐，原文留给用户修正。
        return (!valid_safe_text(value, 128) || value.contains(':'))
            .then_some("setup-admin-actor-invalid");
    }
    if value.is_empty() {
        return Some("admin-field-required");
    }
    if field.maximum_bytes().is_some_and(|maximum| value.len() > maximum) {
        return Some("setup-field-invalid");
    }
    if let Some((minimum, maximum, step)) = field.number_bounds()
        && (!value.bytes().all(|byte| byte.is_ascii_digit())
            || !value
                .parse::<u64>()
                .is_ok_and(|number| (minimum..=maximum).contains(&number) && number % step == 0))
    {
        return Some("setup-field-invalid");
    }
    if SetupField::LISTENER.contains(&field)
        && let (Ok(public), Ok(admin)) = (
            draft.value(SetupField::PublicListenerPort).parse::<u16>(),
            draft.value(SetupField::AdminListenerPort).parse::<u16>(),
        )
        && public == admin
    {
        // 按数值比较，避免前导零绕过两个回环监听器的端口互斥。
        return Some("setup-listener-conflict");
    }
    if field == SetupField::ScheduleOpenTime
        && let Some(key) = schedule_order_issue(draft)
    {
        return Some(key);
    }
    let valid = match field {
        SetupField::ScheduleTimezone => value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+')),
        _ if field.is_weekday() => SetupField::WEEKDAYS.iter().any(|(day, _)| *day == value),
        _ if field.is_schedule_time() => schedule_minute(value).is_some(),
        SetupField::DatabaseHost => database_text(value) && valid_host(value),
        SetupField::DatabaseUsername | SetupField::DatabasePassword => database_text(value),
        SetupField::DatabaseName => database_text(value) && !matches!(value, "." | ".."),
        SetupField::AdminSecret => {
            value.len() >= 32 && !value.bytes().any(|byte| byte.is_ascii_control())
        }
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
        // 不在浏览器复制日志过滤语法的解析器；本地只校验无歧义的文本边界，语法由服务端判定。
        SetupField::LogFilter => !value.contains(['\r', '\n', '\0']),
        _ => true,
    };
    (!valid).then_some("setup-field-invalid")
}

/// 页面只提示本地周内的分钟顺序；IANA 时区有效性和真实排期由服务端决定。
fn schedule_order_issue(draft: &SetupDraft) -> Option<&'static str> {
    let minute = |day, time| {
        let day = SetupField::WEEKDAYS.iter().position(|(value, _)| *value == draft.value(day))?;
        Some(u32::try_from(day).ok()? * 24 * 60 + schedule_minute(draft.value(time))?)
    };
    let preview = minute(SetupField::SchedulePreviewDay, SetupField::SchedulePreviewTime)?;
    let open = minute(SetupField::ScheduleOpenDay, SetupField::ScheduleOpenTime)?;
    let end = minute(SetupField::ScheduleEndDay, SetupField::ScheduleEndTime)?;
    (preview + 120 > open || open >= end).then_some("setup-schedule-order")
}

/// 规范的 `HH:MM`（各两位、补零）是当天的第几分钟；其他写法一律不接受。
fn schedule_minute(value: &str) -> Option<u32> {
    let [hour_tens, hour_ones, b':', minute_tens, minute_ones] = value.as_bytes() else {
        return None;
    };
    let pair = |tens: &u8, ones: &u8| {
        (tens.is_ascii_digit() && ones.is_ascii_digit())
            .then(|| u32::from(tens - b'0') * 10 + u32::from(ones - b'0'))
    };
    let (hour, minute) = (pair(hour_tens, hour_ones)?, pair(minute_tens, minute_ones)?);
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
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
                || matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
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

#[cfg(test)]
mod tests {
    use super::{database_target, issue};
    use crate::surface::setup::draft::{SetupDraft, SetupField};

    #[test]
    fn setup_admin_actor_rejects_noncanonical_text_without_changing_the_input() {
        let mut draft = SetupDraft::default();
        let field = SetupField::AdminActor;
        for valid in ["管理 员".to_owned(), "é".repeat(64)] {
            draft.set(field, valid.clone());
            assert_eq!(issue(&draft, field), None);
            assert_eq!(draft.value(field), valid);
        }
        for invalid in [
            " admin".to_owned(),
            "admin ".to_owned(),
            "\u{2003}admin".to_owned(),
            "a\tmin".to_owned(),
            "a\u{0085}min".to_owned(),
            "admin:owner".to_owned(),
            "é".repeat(65),
            String::new(),
        ] {
            draft.set(field, invalid.clone());
            assert_eq!(issue(&draft, field), Some("setup-admin-actor-invalid"), "{invalid:?}");
            assert_eq!(draft.value(field), invalid);
        }
    }

    #[test]
    fn setup_confirmation_is_revoked_on_each_target_edit() {
        for (field, edited) in [
            (SetupField::DatabaseHost, "::1"),
            (SetupField::DatabasePort, "5433"),
            (SetupField::DatabasePort, "05432"),
            (SetupField::DatabaseName, "other"),
        ] {
            let mut draft = SetupDraft::default();
            let original = draft.value(field).to_owned();
            draft.confirmed_database_target = database_target(&draft);
            assert!(draft.confirmed_database_target.is_some());
            draft.set(SetupField::DatabasePassword, "changed-password".to_owned());
            draft.set(field, original.clone());
            assert!(draft.confirmed_database_target.is_some());
            draft.set(field, edited.to_owned());
            assert!(draft.confirmed_database_target.is_none());
            draft.set(field, original);
            assert!(draft.confirmed_database_target.is_none());
            assert!(database_target(&draft).is_some());
        }
    }

    #[test]
    fn setup_confirmation_requires_a_valid_host_port_and_name() {
        for (field, invalid) in [
            (SetupField::DatabaseHost, "https://database.example.test"),
            (SetupField::DatabasePort, "0"),
            (SetupField::DatabaseName, ".."),
        ] {
            let mut draft = SetupDraft::default();
            assert!(database_target(&draft).is_some());
            draft.set(field, invalid.to_owned());
            assert!(database_target(&draft).is_none());
        }
    }
}
