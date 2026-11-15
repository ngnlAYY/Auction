//! 邮件凭据的玩家名称校验；配置声明的预期名称不构成身份证明。

/// 完整游戏玩家名称按原文保留，禁止首尾空白、控制字符和超长值。
pub fn valid_mail_player_name(value: &str) -> bool {
    auction_domain::valid_text(value, 256)
}

/// 只有邮件 Cookie 可以声明预期名称；空值必须显式使用 None。
pub fn valid_credential_player_name(
    kind: crate::ports::admin::AdminCredentialKind,
    value: Option<&str>,
) -> bool {
    value.is_none_or(|name| {
        kind == crate::ports::admin::AdminCredentialKind::MailCookie && valid_mail_player_name(name)
    })
}
