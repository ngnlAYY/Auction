//! delivery 证明、请求摘要和错误映射。

use auction_application::core::{ApplicationError, RequestHash};

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

pub(super) fn repository_error(message: &str) -> ApplicationError {
    auction_application::core::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
pub(super) fn repository_error_owned(message: &str) -> auction_application::core::RepositoryError {
    auction_application::core::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::core::RepositoryError {
    let context = match &error {
        sqlx::Error::RowNotFound => "delivery 记录不存在",
        _ => "delivery 数据库操作失败",
    };
    auction_application::core::RepositoryError::Failed(crate::database::sql_error_context(
        &error, context,
    ))
}
