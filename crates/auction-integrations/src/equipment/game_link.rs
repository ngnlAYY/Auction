//! 公开页面查看 HentaVerse 装备时使用的短生命周期解密适配器。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use auction_application::{
    ApplicationError, ApplicationFuture, PublicEquipmentGameLink, PublicEquipmentLinkResolver,
    PublicEquipmentLinkSecretReader, PublicEquipmentLinkTarget, RepositoryError,
};

use crate::cryptobox::EquipmentKeyOpener;

/// 把当前公开 lot、密文 owner 与装备数据密钥组合成一次完整游戏 URL 投影。
/// 完整 URL 只进入 P09/P11 lot DTO；解析器在读取公开投影后才打开 key，并立即把经校验的 URL
/// 移交给 HTTP 映射边界。
pub struct EquipmentGameLinkResolver {
    secrets: Arc<dyn PublicEquipmentLinkSecretReader>,
    opener: EquipmentKeyOpener,
}

impl EquipmentGameLinkResolver {
    /// 建立 PostgreSQL 完整档的装备游戏链接解析器。
    pub fn new(
        secrets: Arc<dyn PublicEquipmentLinkSecretReader>,
        opener: EquipmentKeyOpener,
    ) -> Self {
        Self { secrets, opener }
    }
}

impl PublicEquipmentLinkResolver for EquipmentGameLinkResolver {
    fn resolve_public_equipment_links<'a>(
        &'a self,
        targets: &'a [PublicEquipmentLinkTarget],
    ) -> ApplicationFuture<'a, Vec<PublicEquipmentGameLink>> {
        Box::pin(async move {
            if targets.is_empty() {
                return Ok(Vec::new());
            }
            let secrets = &self.secrets;
            let opener = &self.opener;

            let mut equipment_ids = BTreeSet::new();
            for target in targets {
                target.validate().map_err(dependency_error)?;
                equipment_ids.insert(target.equipment_id);
            }
            let requested = equipment_ids.iter().copied().collect::<Vec<_>>();
            let stored = secrets.read_public_equipment_link_secrets(requested).await?;
            let mut secrets_by_owner = BTreeMap::new();
            for secret in stored {
                secret.validate().map_err(dependency_error)?;
                if !equipment_ids.contains(&secret.equipment_id)
                    || secrets_by_owner.insert(secret.equipment_id, secret).is_some()
                {
                    return Err(dependency_error("公开装备链接密钥批次不一致"));
                }
            }
            if secrets_by_owner.len() != equipment_ids.len() {
                return Err(dependency_error("公开装备链接密钥缺失"));
            }

            let mut links = Vec::with_capacity(targets.len());
            for target in targets {
                let secret = secrets_by_owner
                    .get(&target.equipment_id)
                    .ok_or_else(|| dependency_error("公开装备链接 owner 缺失"))?;
                if secret.world_id != target.world_id {
                    return Err(dependency_error("公开装备链接 owner 不一致"));
                }

                let origin = match secret.world_id.as_str() {
                    "main" => "https://hentaiverse.org/equip/",
                    "isekai" => "https://hentaiverse.org/isekai/equip/",
                    _ => return Err(dependency_error("公开装备链接世界无效")),
                };
                let public_url = format!("{origin}{}", secret.external_equipment_id);
                if target.projected_game_equipment_url != public_url {
                    return Err(dependency_error("公开装备链接投影不一致"));
                }

                let key = opener.open(secret.equipment_id, &secret.envelope).map_err(
                    |diagnostic_cause| {
                        safe_diagnostics::warn(
                            "equipment_link",
                            "open_key",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        dependency_error("公开装备链接密钥不可用")
                    },
                )?;
                let link = key
                    .with_bytes(|bytes| {
                        let key = std::str::from_utf8(bytes).ok()?;
                        PublicEquipmentGameLink::new(format!("{public_url}/{key}")).ok()
                    })
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "equipment_link",
                            "build_link",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        dependency_error("公开装备链接密钥无效")
                    })?
                    .ok_or_else(|| dependency_error("公开装备链接格式无效"))?;
                links.push(link);
            }
            Ok(links)
        })
    }
}

fn dependency_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
