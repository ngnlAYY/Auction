//! 外部 Cookie 的导入来源；账号密码只在本次获取期间存在，不成为持久凭据。

use std::fmt;

use crate::{
    AdminCredentialError, AdminCredentialKind, AdminCredentialSecret, ApplicationError,
    ApplicationFuture,
};

/// 一次论坛登录所需的敏感输入；用户名不等于游戏内完整玩家名称。
pub struct AdminCredentialLogin {
    username: String,
    password: AdminCredentialSecret,
}

impl AdminCredentialLogin {
    /// 校验登录字段，保留密码原文；错误不能包含账号或密码。
    pub fn new(username: String, password: String) -> Result<Self, AdminCredentialError> {
        let password = AdminCredentialSecret::new(password)?;
        if !auction_domain::valid_text(&username, 256)
            || password.as_bytes().len() > 4096
            || password.as_bytes().iter().any(u8::is_ascii_control)
        {
            return Err(AdminCredentialError::InvalidRequest);
        }
        Ok(Self { username, password })
    }

    /// 仅交给受限论坛登录适配器，不用于日志或公开投影。
    pub fn username(&self) -> &str {
        &self.username
    }

    /// 只读借用原密码，不能写入账本、数据库或响应。
    pub fn password(&self) -> &[u8] {
        self.password.as_bytes()
    }
}

impl fmt::Debug for AdminCredentialLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 显式选择粘贴 Cookie 或论坛账号登录；两者不能同时提供。
#[derive(Debug)]
pub enum AdminCredentialImportSource {
    Cookie(AdminCredentialSecret),
    ForumLogin(AdminCredentialLogin),
}

/// 集成适配器拥有远端登录及论坛/游戏 Cookie 字段映射，不向应用暴露 HTTP 协议。
pub trait AdminCredentialAcquirer: Send + Sync {
    /// 使用独立用途域的带密钥摘要绑定完整登录输入，禁止持久化可离线猜测的裸密码摘要。
    fn login_digest(&self, login: &AdminCredentialLogin) -> Result<[u8; 32], ApplicationError>;

    /// 获取指定用途的 Cookie；返回值只交给密封适配器，不返回浏览器。
    fn acquire(
        &self,
        kind: AdminCredentialKind,
        source: AdminCredentialImportSource,
    ) -> ApplicationFuture<'_, AdminCredentialSecret>;
}
