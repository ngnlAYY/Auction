//! 数据密钥域的闭集：每个域使用独立父密钥，信封中的 key ID 只能在本域密钥环内解释。
//!
//! 域名字符串同时是敏感 TOML 前缀、密钥环注册表主键和 MoogleMail 诊断归档头部的
//! `key_source` 值。新增域必须同时扩展配置装配、注册表 CHECK 与归档格式，不能只在本枚举
//! 里追加成员。

use std::fmt;

/// 父密钥使用场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataKeyDomain {
    /// 装备短时 key 的信封父密钥。
    Equipment,
    /// 用户 MoogleMail 登录 key 的信封父密钥。
    UserLogin,
    /// 外部 Cookie 凭据的信封父密钥。
    Credential,
    /// MoogleMail 异常诊断归档的独立父密钥。
    MailDiagnostics,
}

impl DataKeyDomain {
    pub const ALL: [Self; 4] =
        [Self::Equipment, Self::UserLogin, Self::Credential, Self::MailDiagnostics];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equipment => "equipment",
            Self::UserLogin => "user_login",
            Self::Credential => "credential",
            Self::MailDiagnostics => "mail_diagnostics",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|domain| domain.as_str() == value)
    }
}

impl fmt::Display for DataKeyDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_strings_are_unique_and_round_trip() {
        let mut seen = Vec::new();
        for domain in DataKeyDomain::ALL {
            assert!(seen.iter().all(|seen| seen != &domain.as_str()));
            seen.push(domain.as_str());
            assert_eq!(DataKeyDomain::parse(domain.as_str()), Some(domain));
        }
        assert_eq!(DataKeyDomain::parse("Equipment"), None);
        assert_eq!(DataKeyDomain::parse("mail-diagnostics"), None);
    }
}
