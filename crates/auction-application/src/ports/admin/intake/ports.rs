//! Admin 装备来源与录入事务的窄端口。

use super::*;

/// 外部 URL/canonical JSON 读取器端口。
pub trait AdminIntakeSourceReader: Send + Sync {
    fn read_url(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;

    fn read_json(
        &self,
        world_id: String,
        equipment: CanonicalEquipment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot>;
}

/// 装备 key 信封加密端口。
pub trait EquipmentKeySealerPort: Send + Sync {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError>;
}

/// Admin 录入事务的窄存储适配器能力。
pub trait AdminIntakeTransaction: Send {
    fn add_admin_intake(
        &mut self,
        write: AdminIntakeWrite,
    ) -> impl Future<Output = Result<AdminIntakeResult, CompetitionTransactionError>> + Send;

    fn append_admin_intake_record(
        &mut self,
        record: AdminIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// Admin 装备录入用例端口。
pub trait AdminIntakePort: Send + Sync {
    fn add_item(&self, command: AdminIntakeCommand) -> ApplicationFuture<'_, AdminIntakeResult>;
}
