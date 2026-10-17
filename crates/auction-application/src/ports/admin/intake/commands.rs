//! Admin 装备录入命令与不含明文 key 的请求摘要。

use super::*;

#[derive(Clone)]
pub struct AdminIntakeCommand {
    pub command_id: CommandId,
    pub world_id: String,
    pub source_kind: Option<AdminIntakeSourceKind>,
    pub equipment_url: Option<String>,
    pub equipment: Option<CanonicalEquipment>,
    pub start_price: i64,
    pub player_id: String,
    pub actor_id: String,
    pub request_id: Option<String>,
}

impl fmt::Debug for AdminIntakeCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminIntakeCommand")
            .field("command_id", &self.command_id)
            .field("world_id", &self.world_id)
            .field("source_kind", &self.source_kind)
            .field("equipment_url", &self.equipment_url)
            .field("equipment", &self.equipment)
            .field("start_price", &self.start_price)
            .field("player_id", &self.player_id)
            .field("actor_id", &self.actor_id)
            .field("request_id", &self.request_id)
            .finish()
    }
}

impl AdminIntakeCommand {
    /// 空 source kind 沿用旧契约的 URL 默认值。
    pub const fn normalized_source_kind(&self) -> AdminIntakeSourceKind {
        match self.source_kind {
            Some(value) => value,
            None => AdminIntakeSourceKind::Url,
        }
    }

    /// 校验不依赖数据库状态的命令字段。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.command_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.start_price <= 0
            || !valid_text(&self.player_id, 256)
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
        {
            return Err(AdminIntakeError::InvalidCommand);
        }
        match self.normalized_source_kind() {
            AdminIntakeSourceKind::Url => {
                if self.equipment.is_some()
                    || self.equipment_url.as_deref().is_none_or(|value| !valid_text(value, 2048))
                {
                    return Err(AdminIntakeError::InvalidCommand);
                }
            }
            AdminIntakeSourceKind::Json => {
                if self.equipment_url.is_some()
                    || self.equipment.as_ref().is_none_or(|value| value.validate().is_err())
                {
                    return Err(AdminIntakeError::InvalidCommand);
                }
            }
        }
        Ok(())
    }

    /// 计算不包含明文 key 的稳定请求摘要。
    /// `request_id` 只属于观测与审计；每次 HTTP 重试都会变化，不能参与业务幂等判等。
    pub fn request_hash(&self, aggregate_id: Uuid) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            source_kind: &'static str,
            world_id: &'a str,
            equipment_url: Option<&'a str>,
            equipment: Option<CanonicalEquipmentDigest<'a>>,
            start_price: i64,
            player_id: &'a str,
            actor_id: &'a str,
        }

        let equipment = self.equipment.as_ref().map(CanonicalEquipment::digest_view);
        RequestHash::from_request(
            "admin_auction_intake_item",
            aggregate_id,
            &Request {
                source_kind: self.normalized_source_kind().as_str(),
                world_id: &self.world_id,
                equipment_url: self.equipment_url.as_deref(),
                equipment,
                start_price: self.start_price,
                player_id: &self.player_id,
                actor_id: &self.actor_id,
            },
        )
    }
}
