use thiserror::Error;

/// Public 会话用例的稳定错误分类。
/// HTTP 层只把这些分类映射为旧契约的状态码；数据库错误仍保留在应用错误的依赖失败分支，
/// 不把具体 SQL 或凭据内容泄漏给浏览器。
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("登录凭据格式无效")]
    InvalidLoginKey,
    #[error("登录凭据无效")]
    InvalidCredentials,
    #[error("会话无效或已过期")]
    Unauthenticated,
    #[error("用户不可创建公开会话")]
    AccountInactive { public_reason: Option<String> },
    #[error("并发公开会话达到上限")]
    SessionLimit,
    #[error("会话配置无效")]
    InvalidConfiguration,
    #[error("公开登录身份无效")]
    InvalidIdentity,
    #[error("公开登录时长偏好无效")]
    InvalidPreference,
    #[error("公开登录时长偏好版本或命令冲突")]
    PreferenceConflict,
}
