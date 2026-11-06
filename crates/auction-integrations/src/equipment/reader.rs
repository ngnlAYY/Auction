//! 装备来源读取器与应用端口适配；负责请求节流、无重定向读取、响应边界、world
//! 路径选择和短生命周期 key 交接，页面解析归档在 `parser`/`model`。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::{
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Instant as StdInstant,
};

use auction_application::admin::{
    AdminIntakeError, AdminIntakeSourceReader, AdminIntakeSourceSnapshot, CanonicalEquipment,
    EquipmentKeySealerPort,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::mail::{
    MailDiagnosticContext, MailDiagnosticOutcome, MailDiagnosticRecorder, MailDiagnosticStage,
    MailEquipmentAttachment, MailEquipmentDiagnosticCategory, MailEquipmentDiagnosticContext,
    MailEquipmentDiagnosticSnapshot, MailEquipmentDiagnosticStage, MailEquipmentEvidenceStatus,
    MailEquipmentObservation, MailEquipmentReadOutcome, MailEquipmentReader, PreparedMailEvidence,
};
use auction_domain::{EquipmentId, EquipmentKey};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use tokio::{
    sync::Mutex,
    time::{Instant, sleep_until, timeout},
};
use url::Url;

use crate::{
    EquipmentKeySealer, EquipmentKeySealerError,
    mooglemail::{
        MoogleMailCookie, MoogleMailCredentialSet, MoogleMailOperation, MoogleMailWorld,
        is_hentaiverse_login_response,
    },
};

use super::{
    config::{DEFAULT_USER_AGENT, EquipmentSourceConfigurationError, EquipmentSourcePolicy},
    model::{EquipmentFacts, canonical_facts, canonical_fingerprint, sha256},
    parser::{EquipmentParseError, parse_equipment_page, parse_equipment_url},
};

mod diagnostics;
mod fetch;

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
    Observed(fetch::EquipmentFetchFailure),
}

impl EquipmentSourceAdapter {
    /// 建立固定 endpoint 的来源适配器。
    /// 传入的 client 应由装配根使用 `build_equipment_http_client` 创建，从而关闭自动重定向
    /// 和环境代理；适配器仍会再次检查最终响应 URL。
    pub fn new(
        client: Client,
        endpoint: Url,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(EquipmentSourceConfigurationError::InvalidEndpoint);
        }
        policy.validate()?;
        Ok(Self {
            client,
            endpoint,
            policy,
            credentials: None,
            diagnostics: None,
            diagnostic_reporter: None,
            next_request_at: Mutex::new(Instant::now()),
        })
    }

    pub fn production(
        client: Client,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        let endpoint = Url::parse("https://hentaiverse.org")
            .map_err(|_| EquipmentSourceConfigurationError::InvalidEndpoint)?;
        Self::new(client, endpoint, policy)
    }

    /// 注入与 MoogleMail 共用的 world-aware 单 Cookie 集合。
    /// HentaiVerse 装备页要求登录；复用同一 Cookie 可以在远端明确返回登录页时通过窄端口
    /// 把数据库凭据标为失效。单 Cookie 失效后不自动切换，且 Cookie 不进入 URL、错误或日志。
    pub fn with_mooglemail_credentials(mut self, credentials: MoogleMailCredentialSet) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// 注入与邮件读取共用的诊断归档器；归档器只接收已脱敏关联字段和加密前的短缓冲。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// 注入装配根拥有的安全日志出口；即使不留存原始响应，也可以记录失败摘要。
    pub fn with_diagnostic_reporter(
        mut self,
        reporter: impl Fn(&MailEquipmentDiagnosticContext, &MailEquipmentDiagnosticSnapshot)
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.diagnostic_reporter = Some(Arc::new(reporter));
        self
    }

    async fn read_url_inner(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        let (target, equipment_id, key) =
            parse_equipment_url(&self.endpoint, &self.policy, &world_id, &equipment_url)?;
        let world = MoogleMailWorld::parse(&world_id).ok_or(AdminIntakeError::InvalidCommand)?;
        let body =
            fetch::SensitiveResponseBody(self.fetch_document_for_world(world, target).await?);
        let facts = parse_equipment_page(&body.0, &equipment_id)
            .map_err(|reason| diagnostics::source_parse_error("html", reason))?;
        let key = EquipmentKey::new(&key).map_err(|_| AdminIntakeError::InvalidSource)?;
        facts_to_snapshot(&world_id, facts, key)
    }

    async fn await_request_slot(&self) {
        let mut next_request_at = self.next_request_at.lock().await;
        let now = Instant::now();
        if *next_request_at > now {
            sleep_until(*next_request_at).await;
        }
        *next_request_at = Instant::now() + self.policy.minimum_request_interval;
    }

    async fn fetch_document_for_world(
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
                let mut body = fetch::SensitiveResponseBody(body);
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

    async fn fetch_document_for_world_observed(
        &self,
        world: MoogleMailWorld,
        target: Url,
    ) -> Result<fetch::EquipmentFetchSuccess, fetch::EquipmentFetchFailure> {
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
                fetch::EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::Timeout,
                    status: fetch::nonzero_status(&status_slot),
                    elapsed_millis: fetch::elapsed_millis(started),
                    body: None,
                }
            })?
            .map_err(|error| match error {
                EquipmentFetchError::Observed(failure) => failure,
                EquipmentFetchError::SessionExpired => fetch::EquipmentFetchFailure {
                    category: MailEquipmentDiagnosticCategory::SessionExpired,
                    status: None,
                    elapsed_millis: None,
                    body: None,
                },
                EquipmentFetchError::Intake(_) => fetch::EquipmentFetchFailure {
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
                fetch::EquipmentFetchFailure {
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
            fetch::EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::Timeout,
                status: fetch::nonzero_status(&status_slot),
                elapsed_millis: fetch::elapsed_millis(started),
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
                    fetch::EquipmentFetchFailure {
                        category: MailEquipmentDiagnosticCategory::CredentialUnavailable,
                        status: fetch::nonzero_status(&status_slot),
                        elapsed_millis: fetch::elapsed_millis(started),
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
            Err(EquipmentFetchError::SessionExpired) => Err(fetch::EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::SessionExpired,
                status: None,
                elapsed_millis: None,
                body: None,
            }),
            Err(EquipmentFetchError::Intake(_)) => Err(fetch::EquipmentFetchFailure {
                category: MailEquipmentDiagnosticCategory::Network,
                status: None,
                elapsed_millis: None,
                body: None,
            }),
        }
    }

    async fn fetch_document_once_observed(
        &self,
        target: Url,
        cookie: Option<&MoogleMailCookie>,
        status_slot: Option<Arc<AtomicU16>>,
    ) -> Result<fetch::EquipmentFetchSuccess, EquipmentFetchError> {
        fetch::document_once(self, target, cookie, status_slot).await
    }

    async fn read_mail_attachment_inner(
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

    async fn read_mail_attachment_diagnostic_inner(
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

    fn read_json_inner(
        &self,
        world_id: String,
        input: CanonicalEquipment,
    ) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
        canonical_json_snapshot(&world_id, input)
    }
}

/// canonical JSON 录入不经网络：W1.10 后 Admin 仍在本进程完成该分支，
/// URL 分支则改为经 Worker control 执行。
pub fn canonical_json_snapshot(
    world_id: &str,
    input: CanonicalEquipment,
) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
    if !matches!(world_id, "main" | "isekai") {
        return Err(AdminIntakeError::InvalidCommand);
    }
    input.validate()?;
    let facts = canonical_facts(&input)
        .map_err(|reason| diagnostics::source_parse_error("canonical_json", reason))?;
    let key = EquipmentKey::new(&input.key).map_err(|_| AdminIntakeError::InvalidSource)?;
    let fingerprint = canonical_fingerprint(world_id, &facts)?;
    let attributes = facts.attributes();
    Ok(AdminIntakeSourceSnapshot {
        adapter: "manual-json-v1".to_owned(),
        source_fingerprint: fingerprint,
        external_equipment_id: facts.external_equipment_id,
        name: facts.name,
        category: facts.category,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        attributes,
        collected_at: Utc::now(),
        key,
    })
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
            self.read_json_inner(world_id, equipment).map_err(ApplicationError::from)
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

fn facts_to_snapshot(
    world_id: &str,
    facts: EquipmentFacts,
    key: EquipmentKey,
) -> Result<AdminIntakeSourceSnapshot, AdminIntakeError> {
    let attributes = facts.attributes();
    let mut fingerprint_input = Vec::new();
    fingerprint_input.extend_from_slice(b"hentaiverse-equipment-v1\0");
    fingerprint_input.extend_from_slice(world_id.as_bytes());
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(facts.external_equipment_id.as_bytes());
    Ok(AdminIntakeSourceSnapshot {
        adapter: "hentaiverse-equipment-v1".to_owned(),
        source_fingerprint: sha256(fingerprint_input),
        external_equipment_id: facts.external_equipment_id,
        name: facts.name,
        category: facts.category,
        level: facts.level.legacy_level(),
        tradeable: facts.tradeable,
        attributes,
        collected_at: Utc::now(),
        key,
    })
}

/// 创建关闭代理和重定向的来源 HTTP client。
pub fn build_equipment_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build()
}
