//! 结算 delivery 的装备 owner 引用解析。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use super::*;

impl MoogleMailDeliveryTransport {
    /// 在任何远端写入前解析并验证 owner UUID、加密信封、装备 key 与外部装备 ID。
    pub(super) async fn resolve_settlement_equipment(
        &self,
        references: &[SettlementEquipmentSecretReference],
    ) -> Result<Vec<String>, DeliveryTransportError> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        let Some(secrets) = self.settlement_equipment_secrets.as_deref() else {
            return Err(equipment_secret_unavailable());
        };
        let Some(opener) = self.settlement_equipment_opener.as_ref() else {
            return Err(equipment_secret_unavailable());
        };
        let mut result = Vec::with_capacity(references.len());
        for reference in references {
            let equipment_uuid = Uuid::parse_str(&reference.id).map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "settlement_delivery",
                    "parse_equipment_id",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                invalid_payload()
            })?;
            let equipment_id = EquipmentId::from_uuid(equipment_uuid);
            let secret = match secrets.read_equipment_secret(equipment_id).await {
                Ok(secret) => secret,
                Err(ApplicationError::FulfillmentEquipmentSecretNotFound) => {
                    return Err(equipment_secret_not_found());
                }
                Err(diagnostic_cause) => {
                    safe_diagnostics::warn(
                        "settlement_delivery",
                        "read_equipment_secret",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    return Err(equipment_secret_unavailable());
                }
            };
            if secret.equipment_id != equipment_id
                || !valid_decimal_id(&secret.external_equipment_id)
                || result.contains(&secret.external_equipment_id)
            {
                return Err(equipment_secret_unavailable());
            }
            // 旧版 resolver 只有在 key 也能按 owner 身份安全打开后才允许暴露外部装备 ID。
            let _key = opener.open(equipment_id, &secret.envelope).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "settlement_delivery",
                    "open_equipment_secret",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                equipment_secret_unavailable()
            })?;
            result.push(secret.external_equipment_id);
        }
        Ok(result)
    }
}
