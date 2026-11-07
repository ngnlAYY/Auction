//! 邮件附件的装备读取：key 交接、world 路径拼接和诊断观测的失败归类。

use auction_application::admin::{AdminIntakeError, AdminIntakeSourceSnapshot};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::{
    MailEquipmentAttachment, MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticContext,
    MailEquipmentDiagnosticStage, MailEquipmentReadOutcome,
};

use crate::{diagnostics as safe_diagnostics, mooglemail::MoogleMailWorld};

use super::super::parser::{parse_equipment_page, parse_equipment_url};
use super::snapshot::facts_to_snapshot;
use super::{EquipmentSourceAdapter, diagnostics};

impl EquipmentSourceAdapter {
    pub(super) async fn read_mail_attachment_inner(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        let key = attachment
            .key
            .with_bytes(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "decode_attachment_key",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                AdminIntakeError::InvalidSource
            })?
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "validate_attachment_encoding",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                AdminIntakeError::InvalidSource
            })?;
        let prefix = if world_id == "isekai" { "/isekai/equip/" } else { "/equip/" };
        let mut target = self.endpoint.clone();
        target.set_path(&format!("{prefix}{}/{key}", attachment.external_equipment_id));
        self.read_url_inner(world_id, target.to_string()).await
    }

    pub(super) async fn read_mail_attachment_diagnostic_inner(
        &self,
        context: MailEquipmentDiagnosticContext,
        attachment: MailEquipmentAttachment,
    ) -> MailEquipmentReadOutcome {
        let invalid = || {
            diagnostics::prepare_observation(
                self,
                context.clone(),
                MailEquipmentDiagnosticStage::Request,
                MailEquipmentDiagnosticCategory::Configuration,
                None,
                None,
                None,
            )
        };
        if !context.validate() || attachment.external_equipment_id != context.external_equipment_id
        {
            return MailEquipmentReadOutcome::Failed {
                error: ApplicationError::from(AdminIntakeError::InvalidCommand),
                observation: invalid(),
            };
        }
        let world = match MoogleMailWorld::parse(&context.world_id) {
            Some(world) => world,
            None => {
                return MailEquipmentReadOutcome::Failed {
                    error: ApplicationError::from(AdminIntakeError::InvalidCommand),
                    observation: invalid(),
                };
            }
        };
        let prefix = if context.world_id == "isekai" { "/isekai/equip/" } else { "/equip/" };
        let target = match attachment.key.with_bytes(|bytes| {
            std::str::from_utf8(bytes)
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "equipment_fetch",
                        "decode_diagnostic_attachment",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    AdminIntakeError::InvalidSource
                })
                .and_then(|key| {
                    let mut target = self.endpoint.clone();
                    target.set_path(&format!("{prefix}{}/{}", context.external_equipment_id, key));
                    // 与手工 URL 读取复用唯一 origin/world/ID/key/allowlist 门，诊断不另开宽松路径。
                    parse_equipment_url(
                        &self.endpoint,
                        &self.policy,
                        &context.world_id,
                        target.as_str(),
                    )
                    .map(|(target, _, _)| target)
                })
        }) {
            Ok(Ok(target)) => target,
            _ => {
                return MailEquipmentReadOutcome::Failed {
                    error: ApplicationError::from(AdminIntakeError::InvalidSource),
                    observation: diagnostics::prepare_observation(
                        self,
                        context,
                        MailEquipmentDiagnosticStage::Request,
                        MailEquipmentDiagnosticCategory::Configuration,
                        None,
                        None,
                        None,
                    ),
                };
            }
        };
        let result = self.fetch_document_for_world_observed(world, target).await;
        let mut fetched = match result {
            Ok(fetched) => fetched,
            Err(mut failure) => {
                let body = std::mem::take(&mut failure.body);
                let observation = diagnostics::prepare_observation(
                    self,
                    context,
                    MailEquipmentDiagnosticStage::Response,
                    failure.category,
                    failure.status,
                    failure.elapsed_millis,
                    body,
                );
                return MailEquipmentReadOutcome::Failed {
                    error: ApplicationError::from(match failure.category {
                        MailEquipmentDiagnosticCategory::Configuration
                        | MailEquipmentDiagnosticCategory::Redirect
                        | MailEquipmentDiagnosticCategory::ResponseTooLarge
                        | MailEquipmentDiagnosticCategory::ContentEncoding
                        | MailEquipmentDiagnosticCategory::HttpStatus
                        | MailEquipmentDiagnosticCategory::ContentType => {
                            AdminIntakeError::InvalidSource
                        }
                        _ => AdminIntakeError::SourceUnavailable,
                    }),
                    observation,
                };
            }
        };
        let response_status = Some(fetched.status);
        let response_elapsed = fetched.elapsed_millis;
        let body = std::mem::take(&mut fetched.body);
        let equipment_id = context.external_equipment_id.clone();
        let parsed = parse_equipment_page(&body, &equipment_id);
        let owned_body = body;
        match parsed {
            Ok(facts) => match facts_to_snapshot(&context.world_id, facts, attachment.key) {
                Ok(snapshot) => MailEquipmentReadOutcome::Loaded {
                    snapshot,
                    observation: diagnostics::prepare_observation(
                        self,
                        context,
                        MailEquipmentDiagnosticStage::Parse,
                        MailEquipmentDiagnosticCategory::ProtocolDrift,
                        response_status,
                        response_elapsed,
                        Some(owned_body),
                    ),
                },
                Err(error) => MailEquipmentReadOutcome::Failed {
                    error: ApplicationError::from(error),
                    observation: diagnostics::prepare_observation(
                        self,
                        context,
                        MailEquipmentDiagnosticStage::Validate,
                        MailEquipmentDiagnosticCategory::SnapshotInvalid,
                        response_status,
                        response_elapsed,
                        Some(owned_body),
                    ),
                },
            },
            Err(reason) => {
                let mut observation = diagnostics::prepare_observation(
                    self,
                    context,
                    MailEquipmentDiagnosticStage::Parse,
                    MailEquipmentDiagnosticCategory::ProtocolDrift,
                    response_status,
                    response_elapsed,
                    Some(owned_body),
                );
                observation.source_detail_code = Some(reason);
                MailEquipmentReadOutcome::Failed {
                    error: ApplicationError::from(diagnostics::source_parse_error("html", reason)),
                    observation,
                }
            }
        }
    }
}
