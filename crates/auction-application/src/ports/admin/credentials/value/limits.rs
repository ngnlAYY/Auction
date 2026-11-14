//! Admin 凭据值对象共用的边界常量和基础校验。

pub(crate) use auction_domain::valid_text;

/// Admin 凭据明文的最大 UTF-8 字节数。
pub const MAX_ADMIN_CREDENTIAL_SECRET_BYTES: usize = 16 * 1024;
/// Admin 凭据列表允许的最大页大小。
pub const MAX_ADMIN_CREDENTIAL_PAGE_SIZE: i64 = 200;
/// Admin 凭据列表允许的最大偏移。
pub const MAX_ADMIN_CREDENTIAL_OFFSET: i64 = 2_147_483_647;

pub(crate) fn valid_time(value: chrono::DateTime<chrono::Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
