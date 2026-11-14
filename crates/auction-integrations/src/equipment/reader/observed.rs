//! 诊断（观测）读取路径：保留状态码、耗时和失败正文，供脱敏证据归档使用。

use std::sync::{Arc, atomic::AtomicU16};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::MailEquipmentDiagnosticCategory;
use tokio::time::timeout;
use url::Url;

use super::http::{
    EquipmentFetchFailure, EquipmentFetchSuccess, document_once, elapsed_millis, nonzero_status,
};
use super::{EquipmentFetchError, EquipmentSourceAdapter};
use crate::{
    mooglemail::{MoogleMailCookie, MoogleMailOperation, MoogleMailWorld},
    safe_diagnostics,
};

impl EquipmentSourceAdapter {
    pub(super) async fn fetch_document_for_world_observed(
        &self,
        world: MoogleMailWorld,
        target: Url,
    ) -> Result<EquipmentFetchSuccess, EquipmentFetchFailure> {
        let Some(credentials) = self.credentials.as_ref() else {
            self.await_request_slot().await;
            let status_slot = Arc::new(AtomicU16::new(0));
            let started = std::time::Instant::now();
            return timeout(
                self.policy.request_timeout,
                self.fetch_document_once_observed(target, None, Some(Arc::clone(&status_slot))),
            )
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "fetch_observed_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::Timeout,
                    status: nonzero_status(&status_slot),
                    elapsed_millis: elapsed_millis(started),
                    body: None,
                }
            })?
            .map_err(|error| match error {
                EquipmentFetchError::Observed(failure) => failure,
                EquipmentFetchError::SessionExpired => EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::SessionExpired,
                    status: None,
                    elapsed_millis: None,
                    body: None,
                },
                EquipmentFetchError::Intake(_) => EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::Network,
                    status: None,
                    elapsed_millis: None,
                    body: None,
                },
            });
        };
        let mut session = credentials
            .begin_read(world, MoogleMailOperation::Equipment)
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "acquire_observed_session",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::CredentialUnavailable,
                    status: None,
                    elapsed_millis: None,
                    body: None,
                }
            })?;
        self.await_request_slot().await;
        let status_slot = Arc::new(AtomicU16::new(0));
        let started = std::time::Instant::now();
        let result = timeout(
            self.policy.request_timeout,
            self.fetch_document_once_observed(
                target,
                Some(session.cookie()),
                Some(Arc::clone(&status_slot)),
            ),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "equipment_fetch",
                "fetch_authenticated_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::Timeout,
                status: nonzero_status(&status_slot),
                elapsed_millis: elapsed_millis(started),
                body: None,
            }
        })?;
        match result {
            Ok(body) => {
                session.check_current().await.map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "equipment_fetch",
                        "check_observed_owner",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                    EquipmentFetchFailure {
                        category: MailEquipmentDiagnosticCategory::CredentialUnavailable,
                        status: nonzero_status(&status_slot),
                        elapsed_millis: elapsed_millis(started),
                        body: None,
                    }
                })?;
                Ok(body)
            }
            Err(EquipmentFetchError::Observed(failure)) => {
                if failure.category == MailEquipmentDiagnosticCategory::SessionExpired {
                    // 单 Cookie 失效后停止；即使回写失败，也保留本次登录页和状态码证据。
                    let _ = session.invalidate_current().await;
                }
                Err(failure)
            }
            Err(EquipmentFetchError::SessionExpired) => Err(EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::SessionExpired,
                status: None,
                elapsed_millis: None,
                body: None,
            }),
            Err(EquipmentFetchError::Intake(_)) => Err(EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::Network,
                status: None,
                elapsed_millis: None,
                body: None,
            }),
        }
    }

    pub(super) async fn fetch_document_once_observed(
        &self,
        target: Url,
        cookie: Option<&MoogleMailCookie>,
        status_slot: Option<Arc<AtomicU16>>,
    ) -> Result<EquipmentFetchSuccess, EquipmentFetchError> {
        document_once(self, target, cookie, status_slot).await
    }
}
