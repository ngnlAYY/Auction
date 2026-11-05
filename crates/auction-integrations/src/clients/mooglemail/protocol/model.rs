//! MoogleMail 世界、操作和短生命周期协议值对象。

use std::{
    fmt,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

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
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }

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

/// 结算世界与 MoogleMail 世界是同一闭集的两种表示，双向转换不会失败。
impl From<auction_domain::SettlementWorld> for MoogleMailWorld {
    fn from(world: auction_domain::SettlementWorld) -> Self {
        match world {
            auction_domain::SettlementWorld::Main => Self::Main,
            auction_domain::SettlementWorld::Isekai => Self::Isekai,
        }
    }
}

impl From<MoogleMailWorld> for auction_domain::SettlementWorld {
    fn from(world: MoogleMailWorld) -> Self {
        match world {
            MoogleMailWorld::Main => Self::Main,
            MoogleMailWorld::Isekai => Self::Isekai,
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
    Inbox,
    /// 已读历史列表；不得当作新入站业务来源。
    ReadMail,
    Detail,
    Sent,
    SentDetail,
    Balance,
    Equipment,
    Identity,
    /// Armory organize 正向库存页面，只读不提交表单。
    Inventory,
    /// Armory organize 固定写入目标；不能接收任意分类或动作参数。
    InventoryOrganize,
    /// 将明确选中的装备移出仓库。
    EquipmentUnstore,
    /// 解除明确选中装备的保护和锁定；不卸下已穿戴装备。
    EquipmentUnlock,
    Compose,
    AttachAdd,
    AttachCod,
    Send,
    ReturnMessage,
    Take,
    /// 丢弃 compose 页当前草稿；只允许由受控维护流程在精确匹配已知投递时使用。
    Discard,
}

impl MoogleMailOperation {
    /// 纯只读观测：收件箱/已读列表、Sent 列表与详情、余额、装备、身份与库存页。
    /// 写入流程中的 compose/detail 页面不在此列，仍占前台令牌。
    pub const fn is_background_read(self) -> bool {
        matches!(
            self,
            Self::Inbox
                | Self::ReadMail
                | Self::Sent
                | Self::SentDetail
                | Self::Balance
                | Self::Equipment
                | Self::Identity
                | Self::Inventory
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::ReadMail => "read_mail",
            Self::Detail => "detail",
            Self::Sent => "sent",
            Self::SentDetail => "sent_detail",
            Self::Balance => "balance",
            Self::Equipment => "equipment",
            Self::Identity => "identity",
            Self::Inventory => "inventory",
            Self::InventoryOrganize => "inventory_organize",
            Self::EquipmentUnstore => "equipment_unstore",
            Self::EquipmentUnlock => "equipment_unlock",
            Self::Compose => "compose",
            Self::AttachAdd => "attach_add",
            Self::AttachCod => "attach_cod",
            Self::Send => "send",
            Self::ReturnMessage => "return_message",
            Self::Take => "attach_remove",
            Self::Discard => "discard",
        }
    }
}

impl fmt::Display for MoogleMailOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 数据库凭据行的稳定身份；只在运行时集合由 `credential_keys` 构造时存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoogleMailPersistedCredential {
    pub(crate) id: uuid::Uuid,
    pub(crate) version: i64,
}

/// 仅在受控边界内临时使用的 MoogleMail Cookie。
#[derive(Clone)]
pub struct MoogleMailCookie(Arc<MoogleMailCookieValue>);

struct MoogleMailCookieValue {
    value: Box<str>,
    observation_key: uuid::Uuid,
    /// 只用于匹配持久扫描检查点；不是证明，也不会写入日志或 DTO。
    persisted: OnceLock<MoogleMailPersistedCredential>,
    revoked: AtomicBool,
}

impl MoogleMailCookie {
    /// 校验 Cookie 行并建立脱敏值对象。
    pub fn new(value: impl Into<String>) -> Result<Self, MoogleMailError> {
        // Cookie 头首尾空白不参与语义；导入时粘贴的空格/制表符在此收口，内部空行仍失败关闭。
        let mut value = value.into();
        if !valid_secret(value.trim(), MAX_COOKIE_BYTES) {
            let mut bytes = value.into_bytes();
            bytes.fill(0);
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        // 未裁剪时直接移交原缓冲；需要裁剪时也清理含完整 Cookie 的旧分配。
        if value.trim().len() != value.len() {
            let trimmed = value.trim().to_owned();
            let mut bytes = std::mem::replace(&mut value, trimmed).into_bytes();
            bytes.fill(0);
        }
        Ok(Self(Arc::new(MoogleMailCookieValue {
            value: value.into_boxed_str(),
            observation_key: uuid::Uuid::new_v4(),
            persisted: OnceLock::new(),
            revoked: AtomicBool::new(false),
        })))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0.value
    }

    /// 同一凭据的所有克隆共享撤销标记和只在内存使用的观测身份。
    pub(crate) fn observation_key(&self) -> uuid::Uuid {
        self.0.observation_key
    }

    /// 绑定数据库凭据身份；同一 Cookie 只会被绑定一次，拒绝无效值。
    pub(crate) fn bind_persisted_credential(&self, id: uuid::Uuid, version: i64) {
        if id.is_nil() || version <= 0 {
            return;
        }
        let _ = self.0.persisted.set(MoogleMailPersistedCredential { id, version });
    }

    /// 没有数据库来源（配置 Cookie）时返回 `None`，调用方只使用进程内检查点。
    pub(crate) fn persisted_credential(&self) -> Option<MoogleMailPersistedCredential> {
        self.0.persisted.get().copied()
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.0.revoked.load(Ordering::Acquire)
    }

    pub(crate) fn revoke(&self) {
        self.0.revoked.store(true, Ordering::Release);
    }
}

impl PartialEq for MoogleMailCookie {
    fn eq(&self, other: &Self) -> bool {
        self.0.value == other.0.value
    }
}

impl Eq for MoogleMailCookie {}

impl Drop for MoogleMailCookieValue {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.value).into_boxed_bytes();
        bytes.fill(0);
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

/// 详情/compose 的 `mmtoken` 或 Armory 的 `postoken`；只用于短生命周期表单。
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

impl Drop for MoogleMailToken {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_boxed_bytes();
        bytes.fill(0);
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

    pub fn clear(&mut self) {
        self.0.fill(0);
    }
}

impl Drop for MoogleMailEquipmentKey {
    fn drop(&mut self) {
        self.clear();
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
    pub(crate) inventory_metadata: bool,
}

impl MoogleMailTarget {
    /// 返回目标操作，不暴露原始 URL、mid 或装备 key。
    pub const fn operation(&self) -> MoogleMailOperation {
        self.operation
    }

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
