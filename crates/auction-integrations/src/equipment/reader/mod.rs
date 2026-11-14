//! 装备来源读取器的模块根：装配适配器、选定 world 路径并暴露应用端口。
//! 请求节流与单次无重定向读取归档在 `http`，普通/观测读取编排在 `fetch`/`observed`，
//! 附件与快照归一在 `attachment`/`snapshot`，页面解析归档在 `parser`/`model`。

mod attachment;
mod client;
mod diagnostics;
mod fetch;
mod http;
mod observed;
mod snapshot;

pub use client::build_equipment_http_client;
pub use snapshot::canonical_json_snapshot;

use std::sync::Arc;

use auction_application::admin::{
    AdminIntakeError, AdminIntakeSourceReader, AdminIntakeSourceSnapshot, CanonicalEquipment,
    EquipmentKeySealerPort,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::mail::{
    MailDiagnosticRecorder, MailEquipmentAttachment, MailEquipmentDiagnosticCategory,
    MailEquipmentDiagnosticContext, MailEquipmentDiagnosticSnapshot, MailEquipmentDiagnosticStage,
    MailEquipmentObservation, MailEquipmentReader,
};
use auction_domain::{EquipmentId, EquipmentKey};
use reqwest::Client;
use tokio::{sync::Mutex, time::Instant};
use url::Url;

use crate::{
    EquipmentKeySealer, EquipmentKeySealerError,
    mooglemail::{MoogleMailCredentialSet, MoogleMailWorld},
};

use super::{
    config::EquipmentSourcePolicy,
    parser::{parse_equipment_page, parse_equipment_url},
};

type DiagnosticReporter =
    dyn Fn(&MailEquipmentDiagnosticContext, &MailEquipmentDiagnosticSnapshot) + Send + Sync;

/// canonical JSON 与旧装备页共用的来源适配器。
pub struct EquipmentSourceAdapter {
    client: Client,
    endpoint: Url,
    policy: EquipmentSourcePolicy,
    credentials: Option<MoogleMailCredentialSet>,
    diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    diagnostic_reporter: Option<Arc<DiagnosticReporter>>,
    next_request_at: Mutex<Instant>,
}

enum EquipmentFetchError {
    Intake(AdminIntakeError),
    SessionExpired,
    Observed(http::EquipmentFetchFailure),
}

impl EquipmentSourceAdapter {
    async fn read_url_inner(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        let (target, equipment_id, key) =
            parse_equipment_url(&self.endpoint, &self.policy, &world_id, &equipment_url)?;
        let world = MoogleMailWorld::parse(&world_id).ok_or(AdminIntakeError::InvalidCommand)?;
        let body = http::SensitiveResponseBody(self.fetch_document_for_world(world, target).await?);
        let facts = parse_equipment_page(&body.0, &equipment_id)
            .map_err(|reason| diagnostics::source_parse_error("html", reason))?;
        let key = EquipmentKey::new(&key).map_err(|_| AdminIntakeError::InvalidSource)?;
        Ok(snapshot::facts_to_snapshot(&world_id, facts, key))
    }
}

impl AdminIntakeSourceReader for EquipmentSourceAdapter {
    fn read_url(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            self.read_url_inner(world_id, equipment_url).await.map_err(ApplicationError::from)
        })
    }

    fn read_json(
        &self,
        world_id: String,
        equipment: CanonicalEquipment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            canonical_json_snapshot(&world_id, equipment).map_err(ApplicationError::from)
        })
    }
}

impl MailEquipmentReader for EquipmentSourceAdapter {
    fn read(
        &self,
        world_id: String,
        attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            self.read_mail_attachment_inner(world_id, attachment)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn read_diagnostic(
        &self,
        context: MailEquipmentDiagnosticContext,
        attachment: MailEquipmentAttachment,
    ) -> auction_application::mail::MailEquipmentReadFuture<'_> {
        Box::pin(self.read_mail_attachment_diagnostic_inner(context, attachment))
    }

    fn retain_failure(
        &self,
        observation: MailEquipmentObservation,
        stage: MailEquipmentDiagnosticStage,
        category: MailEquipmentDiagnosticCategory,
        detail_code: Option<String>,
    ) -> auction_application::mail::MailEquipmentRetentionFuture<'static> {
        Box::pin(diagnostics::retain_failure(
            self.diagnostics.clone(),
            self.diagnostic_reporter.clone(),
            observation,
            stage,
            category,
            detail_code,
        ))
    }
}

impl EquipmentKeySealerPort for EquipmentKeySealer {
    fn seal(
        &self,
        equipment_id: EquipmentId,
        key: &EquipmentKey,
    ) -> Result<auction_domain::EquipmentKeyEnvelope, AdminIntakeError> {
        EquipmentKeySealer::seal(self, equipment_id, key)
            .map_err(|_: EquipmentKeySealerError| AdminIntakeError::KeyUnavailable)
    }
}
