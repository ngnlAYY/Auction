//! Admin Mail owner 共享常量、聚合标识和字段校验。

pub(super) use auction_domain::valid_text;
use uuid::Uuid;

/// Admin 邮箱标识允许的最大字节数。
pub const MAX_ADMIN_MAILBOX_KEY_BYTES: usize = 256;
/// Admin 邮件查询的最大页大小。
pub const MAX_ADMIN_MAIL_PAGE_SIZE: i64 = 100;
/// Admin 邮件数字分页允许的最大偏移。
pub const MAX_ADMIN_MAIL_OFFSET: i64 = 2_147_483_647;
/// Admin 邮件决定码允许的最大字节数。
pub const MAX_ADMIN_MAIL_DECISION_BYTES: usize = 256;
/// Admin 邮件 dashboard 允许展示的最大轮询器数量。
pub const MAX_ADMIN_MAIL_POLLER_COUNT: usize = 64;

/// 生成邮箱范围内稳定的命令聚合标识。
pub fn admin_mail_aggregate_id(mailbox_key: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/admin-mail/{mailbox_key}").as_bytes())
}

pub(super) fn valid_kind(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || matches!(
                value,
                "equipment" | "bid" | "delivery" | "registration" | "rebind" | "unknown"
            )
    })
}

pub(super) fn valid_status(value: Option<&str>) -> bool {
    value
        .is_none_or(|value| value.is_empty() || matches!(value, "accepted" | "rejected" | "failed"))
}

pub(super) fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        value.is_empty()
            || (value.len() <= maximum
                && value == value.trim()
                && !value.chars().any(char::is_control))
    })
}

pub(super) fn valid_string_list(values: &[String]) -> bool {
    values.len() <= 1000 && values.iter().all(|value| valid_text(value, 512))
}
