//! 公开页面查看游戏装备时使用的密文读取适配器。

use auction_application::auction::ApplicationFuture;
use auction_application::public::{PublicEquipmentLinkSecret, PublicEquipmentLinkSecretReader};
use auction_domain::EquipmentId;

use crate::database::DatabaseStore;
use crate::database::auction::equipment_secrets::read_stored_equipment_secrets_postgres;
use crate::database::storage_error;

impl PublicEquipmentLinkSecretReader for DatabaseStore {
    fn read_public_equipment_link_secrets(
        &self,
        equipment_ids: Vec<EquipmentId>,
    ) -> ApplicationFuture<'_, Vec<PublicEquipmentLinkSecret>> {
        Box::pin(async move {
            let stored = read_stored_equipment_secrets_postgres(self.pool(), &equipment_ids)
                .await
                .map_err(|_| storage_error("公开装备链接密钥读取失败"))?;
            stored
                .into_iter()
                .map(|stored| {
                    let secret = PublicEquipmentLinkSecret {
                        equipment_id: stored.equipment_id,
                        world_id: stored.world_id,
                        external_equipment_id: stored.external_equipment_id,
                        envelope: stored.envelope,
                    };
                    secret.validate().map_err(storage_error)?;
                    Ok(secret)
                })
                .collect()
        })
    }
}
