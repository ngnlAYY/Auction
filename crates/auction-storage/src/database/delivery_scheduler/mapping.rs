//! delivery 证明、请求摘要和错误映射。

#[cfg(feature = "postgres")]
use serde::Serialize;

use super::*;

#[cfg(feature = "postgres")]
#[derive(Serialize)]
pub(super) struct DeliveryProof<'a> {
    pub(super) external_reference: &'a str,
    pub(super) result_metadata: &'a serde_json::Value,
}

#[cfg(feature = "postgres")]
pub(super) fn request_hash(
    value: Vec<u8>,
) -> Result<RequestHash, auction_application::RepositoryError> {
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
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
pub(super) fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "delivery 记录不存在".to_owned(),
        _ => "delivery 数据库操作失败".to_owned(),
    })
}
