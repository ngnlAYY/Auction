//! delivery 证明、请求摘要和错误映射。

use auction_application::core::RequestHash;

#[cfg(feature = "postgres")]
pub(super) fn request_hash(
    value: Vec<u8>,
) -> Result<RequestHash, auction_application::core::RepositoryError> {
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error_owned("delivery request digest 长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error_owned("delivery request digest 不能为零"));
    }
    Ok(RequestHash::new(bytes))
}

pub(super) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) use crate::database::repository_failure as repository_error;

#[cfg(feature = "postgres")]
pub(super) use crate::database::repository_error as repository_error_owned;

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::core::RepositoryError {
    crate::database::storage_failure(&error, "delivery 记录不存在", "delivery 数据库操作失败")
}
