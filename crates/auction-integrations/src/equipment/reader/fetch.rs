//! 普通（非诊断）装备读取的编排：节流、单次请求超时、凭据会话交接和错误归类。

use auction_application::admin::AdminIntakeError;
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::MailEquipmentDiagnosticCategory;
use tokio::time::timeout;
use url::Url;

use crate::{
    diagnostics as safe_diagnostics,
    mooglemail::{MoogleMailCookie, MoogleMailOperation, MoogleMailWorld},
};

use super::http::SensitiveResponseBody;
use super::{EquipmentFetchError, EquipmentSourceAdapter};

impl EquipmentSourceAdapter {
    pub(super) async fn fetch_document_for_world(
        &self,
        world: MoogleMailWorld,
        target: Url,
    ) -> Result<Vec<u8>, AdminIntakeError> {
        let Some(credentials) = self.credentials.as_ref() else {
            return match self.fetch_document_attempt(target, None).await {
                Ok(body) => Ok(body),
                Err(EquipmentFetchError::Intake(error)) => Err(error),
                Err(EquipmentFetchError::SessionExpired) => {
                    Err(AdminIntakeError::SourceUnavailable)
                }
                Err(EquipmentFetchError::Observed(_)) => {
                    return Err(AdminIntakeError::SourceUnavailable);
                }
            };
        };
        let mut session = credentials
            .begin_read(world, MoogleMailOperation::Equipment)
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "acquire_session",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                AdminIntakeError::SourceUnavailable
            })?;
        match self.fetch_document_attempt(target, Some(session.cookie())).await {
            Ok(body) => {
                let mut body = SensitiveResponseBody(body);
                session.check_current().await.map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "equipment_fetch",
                        "check_response_owner",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                    AdminIntakeError::SourceUnavailable
                })?;
                Ok(std::mem::take(&mut body.0))
            }
            Err(EquipmentFetchError::Intake(error)) => Err(error),
            Err(EquipmentFetchError::SessionExpired) => {
                let _ = session.invalidate_current().await;
                Err(AdminIntakeError::SourceUnavailable)
            }
            Err(EquipmentFetchError::Observed(_)) => Err(AdminIntakeError::SourceUnavailable),
        }
    }

    async fn fetch_document_attempt(
        &self,
        target: Url,
        cookie: Option<&MoogleMailCookie>,
    ) -> Result<Vec<u8>, EquipmentFetchError> {
        self.await_request_slot().await;
        timeout(self.policy.request_timeout, self.fetch_document_once(target, cookie))
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "fetch_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentFetchError::Intake(AdminIntakeError::SourceUnavailable)
            })?
    }

    async fn fetch_document_once(
        &self,
        target: Url,
        cookie: Option<&MoogleMailCookie>,
    ) -> Result<Vec<u8>, EquipmentFetchError> {
        match self.fetch_document_once_observed(target, cookie, None).await {
            Ok(mut body) => Ok(std::mem::take(&mut body.body)),
            Err(EquipmentFetchError::SessionExpired) => Err(EquipmentFetchError::SessionExpired),
            Err(EquipmentFetchError::Observed(failure))
                if failure.category == MailEquipmentDiagnosticCategory::SessionExpired =>
            {
                Err(EquipmentFetchError::SessionExpired)
            }
            Err(EquipmentFetchError::Observed(failure)) => {
                let error = match failure.category {
                    MailEquipmentDiagnosticCategory::RateLimited
                    | MailEquipmentDiagnosticCategory::ServerError
                    | MailEquipmentDiagnosticCategory::Timeout
                    | MailEquipmentDiagnosticCategory::CredentialUnavailable
                    | MailEquipmentDiagnosticCategory::Network => {
                        AdminIntakeError::SourceUnavailable
                    }
                    _ => AdminIntakeError::InvalidSource,
                };
                Err(EquipmentFetchError::Intake(error))
            }
            Err(EquipmentFetchError::Intake(error)) => Err(EquipmentFetchError::Intake(error)),
        }
    }
}
