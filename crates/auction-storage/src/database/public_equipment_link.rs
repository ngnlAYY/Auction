//! 公开页面查看游戏装备时使用的密文读取适配器。

#[cfg(feature = "postgres")]
use auction_application::RepositoryError;
use auction_application::{
    ApplicationError, ApplicationFuture, PublicEquipmentLinkSecret, PublicEquipmentLinkSecretReader,
};
use auction_domain::EquipmentId;

use super::DatabaseStore;
#[cfg(feature = "postgres")]
use super::equipment_secrets::read_stored_equipment_secrets_postgres;

impl PublicEquipmentLinkSecretReader for DatabaseStore {
    fn read_public_equipment_link_secrets(
        &self,
        equipment_ids: Vec<EquipmentId>,
    ) -> ApplicationFuture<'_, Vec<PublicEquipmentLinkSecret>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    let stored =
                        read_stored_equipment_secrets_postgres(store.pool(), &equipment_ids)
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
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
fn storage_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
