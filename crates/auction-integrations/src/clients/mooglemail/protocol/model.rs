//! MoogleMail 世界、操作和短生命周期协议值对象。

use std::fmt;

use url::Url;

use super::super::{MoogleMailError, MoogleMailErrorCategory};
use super::validation::valid_secret;
use super::{MAX_COOKIE_BYTES, MAX_TOKEN_BYTES};

/// MoogleMail 的游戏世界闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoogleMailWorld {
    /// 主世界，对应根路径。
    Main,
    /// Isekai 世界，对应 `/isekai/` 路径。
    Isekai,
}

impl MoogleMailWorld {
    /// 返回外部协议中的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }

    /// 解析外部协议中的世界名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "isekai" => Some(Self::Isekai),
            _ => None,
        }
    }

    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Main => "/",
            Self::Isekai => "/isekai/",
        }
    }
}

impl fmt::Display for MoogleMailWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// MoogleMail 读取和写入协议的操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoogleMailOperation {
    /// 收件箱分页。
    Inbox,
    /// 单封收件箱详情。
    Detail,
    /// 已发送邮件分页。
    Sent,
    /// 单封已发送邮件详情。
    SentDetail,
    /// Item Shop 余额页面。
    Balance,
    /// 装备详情页面。
    Equipment,
    /// 新邮件页面。
    Compose,
    /// compose 页面附加 Credits。
    AttachAdd,
    /// compose 页面设置 CoD。
    AttachCod,
    /// compose 页面最终发送。
    Send,
    /// 详情页面退回整封邮件。
    ReturnMessage,
    /// 详情页面领取全部附件。
    Take,
}

impl MoogleMailOperation {
    /// 返回稳定的操作名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Detail => "detail",
            Self::Sent => "sent",
            Self::SentDetail => "sent_detail",
            Self::Balance => "balance",
            Self::Equipment => "equipment",
            Self::Compose => "compose",
            Self::AttachAdd => "attach_add",
            Self::AttachCod => "attach_cod",
            Self::Send => "send",
            Self::ReturnMessage => "return_message",
            Self::Take => "attach_remove",
        }
    }
}

impl fmt::Display for MoogleMailOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 仅在受控边界内临时使用的 MoogleMail Cookie。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailCookie(Box<str>);

impl MoogleMailCookie {
    /// 校验 Cookie 行并建立脱敏值对象。
    pub fn new(value: impl Into<String>) -> Result<Self, MoogleMailError> {
        let value = value.into();
        if !valid_secret(&value, MAX_COOKIE_BYTES) {
            let mut bytes = value.into_bytes();
            bytes.fill(0);
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MoogleMailCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 详情页面中的短生命周期 `mmtoken`。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailToken(Box<str>);

impl MoogleMailToken {
    pub(crate) fn new(value: &str) -> Option<Self> {
        valid_secret(value, MAX_TOKEN_BYTES).then(|| Self(value.to_owned().into_boxed_str()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MoogleMailToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// MoogleMail 装备附件的 10 位十六进制 key。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailEquipmentKey([u8; 10]);

impl MoogleMailEquipmentKey {
    /// 校验并规范化装备 key 为小写 ASCII。
    pub fn new(value: &str) -> Option<Self> {
        if value.len() != 10 || !value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut normalized = [0_u8; 10];
        for (index, byte) in value.bytes().enumerate() {
            normalized[index] = byte.to_ascii_lowercase();
        }
        Some(Self(normalized))
    }

    /// 只在构造远端装备 URL 的同步边界内暴露 key。
    pub(crate) fn expose(&self) -> &str {
        // `new` 已经保证每个字节都是 ASCII hex；这里使用一次静态转换避免复制 secret。
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// 尽力清除当前内存中的 key。
    pub fn clear(&mut self) {
        self.0.fill(0);
    }
}

impl fmt::Debug for MoogleMailEquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailEquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 由 URL builder 构造的不透明 MoogleMail 目标。
#[derive(Clone)]
pub struct MoogleMailTarget {
    pub(crate) operation: MoogleMailOperation,
    pub(crate) world: MoogleMailWorld,
    pub(crate) url: Url,
    pub(crate) origin: String,
}

impl MoogleMailTarget {
    /// 返回目标操作，不暴露原始 URL、mid 或装备 key。
    pub const fn operation(&self) -> MoogleMailOperation {
        self.operation
    }

    /// 返回目标世界。
    pub const fn world(&self) -> MoogleMailWorld {
        self.world
    }
}

impl fmt::Debug for MoogleMailTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailTarget")
            .field("operation", &self.operation)
            .field("world", &self.world)
            .finish()
    }
}

impl fmt::Display for MoogleMailTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MoogleMail target: operation={} world={}", self.operation, self.world)
    }
}
