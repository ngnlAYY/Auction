//! 公开页面查看游戏装备时使用的窄端口和脱敏值对象。

use std::fmt;

use auction_domain::{EquipmentId, EquipmentKeyEnvelope};
use uuid::Uuid;

use crate::ApplicationFuture;

/// Public 链接解析所需的加密装备事实；该结构不包含明文 key。
#[derive(Clone)]
pub struct PublicEquipmentLinkSecret {
    pub equipment_id: EquipmentId,
    pub world_id: String,
    pub external_equipment_id: String,
    pub envelope: EquipmentKeyEnvelope,
}

impl PublicEquipmentLinkSecret {
    /// 拒绝无法安全构造 HentaVerse 装备链接的持久化投影。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.equipment_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.external_equipment_id.is_empty()
            || self.external_equipment_id.len() > 32
            || !self.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
            || self.envelope.validate().is_err()
        {
            return Err("公开装备链接密钥投影无效");
        }
        Ok(())
    }
}

impl fmt::Debug for PublicEquipmentLinkSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicEquipmentLinkSecret")
            .field("equipment_id", &self.equipment_id)
            .field("world_id", &self.world_id)
            .field("external_equipment_id", &self.external_equipment_id)
            .field("envelope", &"[REDACTED]")
            .finish()
    }
}

/// 只允许 Public lot 列表/详情 HTTP 映射边界消费的完整游戏装备 URL。
///
/// 该类型不实现 `Display` 或 serde，`Debug` 也始终脱敏，避免完整 key 被日志、JSON 或审计
/// 辅助结构无意复制。调用方只能在构造 P09/P11 DTO 时取得所有权。
pub struct PublicEquipmentGameLink(String);

impl PublicEquipmentGameLink {
    /// 校验固定 HTTPS origin、世界路径、十进制装备 ID 和 10 位十六进制 key。
    pub fn new(value: String) -> Result<Self, &'static str> {
        let path = value
            .strip_prefix("https://hentaiverse.org/equip/")
            .or_else(|| value.strip_prefix("https://hentaiverse.org/isekai/equip/"))
            .ok_or("公开装备游戏链接 origin 无效")?;
        let mut parts = path.split('/');
        let equipment_id = parts.next().unwrap_or_default();
        let key = parts.next().unwrap_or_default();
        if equipment_id.is_empty()
            || equipment_id.len() > 32
            || !equipment_id.bytes().all(|byte| byte.is_ascii_digit())
            || key.len() != 10
            || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
            || parts.next().is_some()
        {
            return Err("公开装备游戏链接路径无效");
        }
        Ok(Self(value))
    }

    /// 把已校验 URL 移交给唯一的 P09/P11 HTTP DTO 映射边界。
    pub fn into_url(self) -> String {
        self.0
    }
}

impl fmt::Debug for PublicEquipmentGameLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 完整游戏装备 URL 解析所需的公开 lot 内部目标；不进入 HTTP DTO。
#[derive(Clone)]
pub struct PublicEquipmentLinkTarget {
    pub lot_id: Uuid,
    pub equipment_id: EquipmentId,
    pub world_id: String,
    pub projected_game_equipment_url: String,
}

impl PublicEquipmentLinkTarget {
    /// 校验 lot/装备 owner、世界和不含 key 的权威公开投影。
    pub fn validate(&self) -> Result<(), &'static str> {
        let prefix = match self.world_id.as_str() {
            "main" => "https://hentaiverse.org/equip/",
            "isekai" => "https://hentaiverse.org/isekai/equip/",
            _ => return Err("公开装备链接目标世界无效"),
        };
        let external_id = self
            .projected_game_equipment_url
            .strip_prefix(prefix)
            .ok_or("公开装备链接目标投影无效")?;
        if self.lot_id.is_nil()
            || self.equipment_id.is_nil()
            || external_id.is_empty()
            || external_id.len() > 32
            || !external_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("公开装备链接目标身份无效");
        }
        Ok(())
    }
}

/// 从权威存储批量读取装备密文信封；SQLite Lite 明确不实现该能力。
pub trait PublicEquipmentLinkSecretReader: Send + Sync {
    fn read_public_equipment_link_secrets(
        &self,
        equipment_ids: Vec<EquipmentId>,
    ) -> ApplicationFuture<'_, Vec<PublicEquipmentLinkSecret>>;
}

/// 为已经从权威投影读取并校验的一批公开 lot 解析同序完整游戏装备 URL。
pub trait PublicEquipmentLinkResolver: Send + Sync {
    fn resolve_public_equipment_links<'a>(
        &'a self,
        targets: &'a [PublicEquipmentLinkTarget],
    ) -> ApplicationFuture<'a, Vec<PublicEquipmentGameLink>>;
}
