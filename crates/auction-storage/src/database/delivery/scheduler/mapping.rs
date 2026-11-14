//! delivery 证明、请求摘要和错误映射。

use auction_application::core::RequestHash;

pub(super) fn request_hash(
    value: Vec<u8>,
) -> Result<RequestHash, auction_application::core::RepositoryError> {
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error("delivery request digest 长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error("delivery request digest 不能为零"));
    }
    Ok(RequestHash::new(bytes))
}

pub(super) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) use crate::database::storage_error;

pub(super) use crate::database::repository_error;

pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::core::RepositoryError {
    crate::database::storage_failure(&error, "delivery 记录不存在", "delivery 数据库操作失败")
}
