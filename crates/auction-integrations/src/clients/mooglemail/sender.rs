//! MoogleMail 分步写入与发送后 Sent 完成证明。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use auction_application::{
    MailScanCheckpoint, MailScanEvent, MailScanObserver, MailScanWatermark,
    advance_mail_scan_watermark,
};
use chrono::Utc;

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

mod baseline;
mod equipment;
mod maintenance;
mod proof;

pub(crate) use baseline::MoogleMailSentBaseline;

pub use maintenance::{
    MoogleMailComposeSnapshot, MoogleMailDiscardOutcome, MoogleMailSentListEntry,
    MoogleMailSentObservation,
};

use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailInventoryCategory, MoogleMailMessageFields, MoogleMailOperation, MoogleMailParser,
    MoogleMailSendResponseFailure, MoogleMailWorld, protocol::valid_decimal_id,
};

const MAX_SEND_ATTACHMENTS: usize = 1000;

/// 已证明 Sent 基线的复用窗口。过期只会退回完整扫描，不改变任何证明。
const SENT_BASELINE_REUSE_WINDOW: Duration = Duration::from_secs(600);

/// 一封待发送 MoogleMail 的业务无关输入。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSendRequest {
    pub world: MoogleMailWorld,
    pub message: MoogleMailMessageFields,
    pub credits: i64,
    /// 要设置的 CoD 数量；零表示不设置 CoD。
    pub cod: i64,
    pub equipment_ids: Vec<String>,
    /// Armory 读取提示；只减少无效页面读取，未命中或无法识别时回退完整七类扫描。
    pub category: Option<MoogleMailInventoryCategory>,
}

impl MoogleMailSendRequest {
    pub fn validate(&self) -> bool {
        self.message.validate()
            && self.credits >= 0
            && self.cod >= 0
            && self.equipment_ids.len() <= MAX_SEND_ATTACHMENTS
            && self.equipment_ids.iter().all(|id| valid_decimal_id(id))
            && self.equipment_ids.iter().collect::<HashSet<_>>().len() == self.equipment_ids.len()
            && (self.cod == 0 || self.credits > 0 || !self.equipment_ids.is_empty())
    }
}

impl fmt::Debug for MoogleMailSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSendRequest")
            .field("world", &self.world)
            .field("message", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("equipment_count", &self.equipment_ids.len())
            .field("category", &self.category)
            .finish()
    }
}

/// 已经由新增 Sent 邮件的身份、正文和精确资产证明的寄送结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSendReceipt {
    /// 通过发送前后 Sent 差异核对得到的真实远端邮件 ID。
    pub external_mail_id: String,
}

/// 最终 send POST 的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSendOutcome {
    /// 新增 Sent 邮件已通过完整内容核验。
    Sent(MoogleMailSendReceipt),
    /// compose 初始页无法证明为干净状态；`dirty` 表示页面可解析但仍有草稿资产。
    PreflightUnproven { dirty: bool },
    /// 远端可见文字明确报告收件人不存在。
    RecipientNotFound { returned_mail_id: Option<String> },
    /// 远端明确拒绝自寄；已经暂存的资产仍须人工核对，不能当作交付成功。
    RecipientIsSender { assets_staged: bool },
    /// 远端明确拒绝发送；附件已经分步放入 compose 时不能自动重试。
    Rejected { assets_staged: bool },
}

/// 负责把一封 typed message 编排为受证明的多步 compose 写入。
#[derive(Clone)]
pub struct MoogleMailSender {
    client: MoogleMailClient,
    parser: MoogleMailParser,
    verification_pages: usize,
    verification_budget: Duration,
    sent_baseline_mode: auction_application::SentBaselineMode,
    sent_baseline_observer: Arc<dyn auction_application::SentBaselineObserver>,
    /// 最近一次已完整证明的 Sent 摘要；只作排除集，缺失条目最多增加候选详情读取。
    sent_baselines: Arc<Mutex<HashMap<MoogleMailWorld, CachedSentBaseline>>>,
    /// Sent 扫描水位；只缩短下一轮前缀，重启即回退完整扫描。
    sent_scans: Arc<Mutex<HashMap<MoogleMailWorld, CachedSentScan>>>,
    /// Read Mail 观测水位；独立于投递游标，只用于维护观察计数。
    read_scans: Arc<Mutex<HashMap<MoogleMailWorld, CachedReadScan>>>,
    scan_observer: Arc<dyn MailScanObserver>,
}

struct CachedSentScan {
    credential: uuid::Uuid,
    scope: super::client::MoogleMailSentScope,
    checkpoint: MailScanCheckpoint,
}

struct CachedReadScan {
    credential: uuid::Uuid,
    checkpoint: MailScanCheckpoint,
}

struct CachedSentBaseline {
    credential: uuid::Uuid,
    scope: super::client::MoogleMailSentScope,
    seen: HashSet<String>,
    captured: Instant,
}

impl MoogleMailSender {
    /// 记录扫描预算实际耗尽；只读同步指标，不改变截止点或重试。
    pub(super) fn observe_budget(
        &self,
        world: MoogleMailWorld,
        phase: auction_application::MailPhase,
        budget: auction_application::MailBudget,
    ) {
        self.client.external_metrics().observe(
            auction_application::ExternalMetricsEvent::MailBudgetExhausted {
                world: crate::clients::metrics::mail_world(world),
                phase,
                budget,
            },
        );
    }

    /// 从既有扫描错误闭集映射预算耗尽类别，不把普通读取失败算成预算。
    pub(super) fn observe_budget_error(
        &self,
        world: MoogleMailWorld,
        phase: auction_application::MailPhase,
        error: &MoogleMailError,
    ) {
        match error.category {
            MoogleMailErrorCategory::SentScanTimeout => {
                self.observe_budget(world, phase, auction_application::MailBudget::Time);
            }
            MoogleMailErrorCategory::SentScanPageLimit => {
                self.observe_budget(world, phase, auction_application::MailBudget::Pages);
            }
            _ => {}
        }
    }

    /// 使用共享 MoogleMail client 构造发送编排器。
    pub fn new(client: MoogleMailClient) -> Self {
        Self {
            client,
            parser: MoogleMailParser,
            verification_pages: 100,
            verification_budget: Duration::from_secs(30),
            sent_baseline_mode: auction_application::SentBaselineMode::Shadow,
            sent_baseline_observer: auction_application::noop_sent_baseline_observer(),
            sent_baselines: Arc::new(Mutex::new(HashMap::new())),
            sent_scans: Arc::new(Mutex::new(HashMap::new())),
            read_scans: Arc::new(Mutex::new(HashMap::new())),
            scan_observer: auction_application::noop_mail_scan_observer(),
        }
    }

    /// 设置发送前后 Sent 核对的页数和总预算。
    pub fn with_send_verification(
        mut self,
        maximum_pages: usize,
        budget: Duration,
    ) -> Result<Self, MoogleMailError> {
        if maximum_pages == 0 || maximum_pages > 100 || budget.is_zero() {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Send,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        self.verification_pages = maximum_pages;
        self.verification_budget = budget;
        Ok(self)
    }

    /// 复用最近一次完整证明的 Sent 摘要；凭据不同或超过窗口时必须重新扫描。
    /// 基线只是排除集：缺失条目最多增加候选详情读取，绝不放行未经验证的正文或资产。
    pub(crate) fn cached_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<HashSet<String>> {
        let cache = self
            .sent_baselines
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_baseline",
                    "lock_cached_baseline",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        let entry = cache.get(&world)?;
        (entry.credential == cookie.observation_key()
            && self
                .client
                .check_sent_scope(cookie, &entry.scope)
                .inspect_err(|cause| {
                    safe_diagnostics::debug(
                        "mooglemail_baseline",
                        "check_cached_scope",
                        safe_diagnostics::project(cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                })
                .is_ok()
            && entry.captured.elapsed() <= SENT_BASELINE_REUSE_WINDOW)
            .then(|| entry.seen.clone())
    }

    /// 由发送前完整枚举或发送后完整排除集与前缀的并集写入；单独水位不能充当完整缓存。
    pub(crate) fn remember_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        seen: HashSet<String>,
    ) {
        let Ok(scope) = self.client.current_sent_scope(cookie, world).inspect_err(|cause| {
            safe_diagnostics::debug(
                "mooglemail_baseline",
                "read_cache_scope",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        if let Ok(mut cache) = self.sent_baselines.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_baseline",
                "lock_baseline_store",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) {
            cache.insert(
                world,
                CachedSentBaseline {
                    credential: cookie.observation_key(),
                    scope,
                    seen,
                    captured: Instant::now(),
                },
            );
        }
    }

    pub(crate) fn forget_sent_baseline(&self, world: MoogleMailWorld) {
        if let Ok(mut cache) = self.sent_baselines.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_baseline",
                "lock_baseline_invalidation",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) {
            cache.remove(&world);
        }
    }

    /// 注入扫描观测；观察失败不改变任何证明或回退路径。
    pub fn with_mail_scan_observer(mut self, observer: Arc<dyn MailScanObserver>) -> Self {
        self.scan_observer = observer;
        self
    }

    fn observe_scan(&self, world: MoogleMailWorld, event: MailScanEvent) {
        self.scan_observer.observe(world.as_str(), event);
    }

    /// 只有凭据与读取代次仍一致的水位才可用于下一轮前缀。
    pub(crate) fn sent_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<MailScanCheckpoint> {
        let cache = self
            .sent_scans
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_sent_scan",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        let entry = cache.get(&world)?;
        (entry.credential == cookie.observation_key()
            && self
                .client
                .check_sent_scope(cookie, &entry.scope)
                .inspect_err(|cause| {
                    safe_diagnostics::debug(
                        "mooglemail_scan",
                        "check_sent_scan_scope",
                        safe_diagnostics::project(cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                })
                .is_ok())
        .then(|| entry.checkpoint.clone())
    }

    /// 记录 Sent 水位；`full` 才推进每日校验时间。水位缺失或作用域变化时只会回退完整扫描。
    pub(crate) fn remember_sent_scan(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        newest: Option<MailScanWatermark>,
        full: bool,
    ) {
        let Ok(scope) = self.client.current_sent_scope(cookie, world).inspect_err(|cause| {
            safe_diagnostics::debug(
                "mooglemail_scan",
                "read_sent_scan_scope",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        let Ok(mut cache) = self.sent_scans.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_scan",
                "lock_sent_scan_store",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        let previous = cache.get(&world).filter(|entry| {
            entry.credential == cookie.observation_key()
                && self.client.check_sent_scope(cookie, &entry.scope).is_ok()
        });
        let watermark = newest
            .map(|newest| {
                advance_mail_scan_watermark(
                    previous.and_then(|entry| entry.checkpoint.watermark.as_ref()),
                    &newest,
                )
            })
            .or_else(|| previous.and_then(|entry| entry.checkpoint.watermark.clone()));
        let last_full_scan_at = if full {
            Some(Utc::now())
        } else {
            previous.and_then(|entry| entry.checkpoint.last_full_scan_at)
        };
        cache.insert(
            world,
            CachedSentScan {
                credential: cookie.observation_key(),
                scope,
                checkpoint: MailScanCheckpoint { watermark, last_full_scan_at },
            },
        );
    }

    /// Read Mail 水位只绑定凭据；维护观察轮次不携带 Sent 作用域。
    pub(crate) fn read_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<MailScanCheckpoint> {
        let cache = self
            .read_scans
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_read_scan",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        cache
            .get(&world)
            .filter(|entry| entry.credential == cookie.observation_key())
            .map(|entry| entry.checkpoint.clone())
    }

    pub(crate) fn remember_read_scan(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        newest: Option<MailScanWatermark>,
        full: bool,
    ) {
        let Ok(mut cache) = self.read_scans.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_scan",
                "lock_read_scan_store",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        let previous =
            cache.get(&world).filter(|entry| entry.credential == cookie.observation_key());
        let watermark = newest
            .map(|newest| {
                advance_mail_scan_watermark(
                    previous.and_then(|entry| entry.checkpoint.watermark.as_ref()),
                    &newest,
                )
            })
            .or_else(|| previous.and_then(|entry| entry.checkpoint.watermark.clone()));
        let last_full_scan_at = if full {
            Some(Utc::now())
        } else {
            previous.and_then(|entry| entry.checkpoint.last_full_scan_at)
        };
        cache.insert(
            world,
            CachedReadScan {
                credential: cookie.observation_key(),
                checkpoint: MailScanCheckpoint { watermark, last_full_scan_at },
            },
        );
    }

    /// 在持久 sending fence 前只读检查 compose；调用方须持有直到 send 完成的同一凭据 guard。
    /// 不清空草稿，也不把历史任务的未知结果改为未发送。
    pub(crate) async fn preflight(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeSnapshot, MoogleMailError> {
        self.inspect_compose(cookie, world).await
    }

    /// 发送失败后按只读方式回读 compose，区分“邮件内容/草稿问题”和其他远端问题。
    /// 只记录页面是否干净与资产计数，不把主题、正文或收件人写入日志。
    async fn diagnose_after_failure(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        expected: &MoogleMailSendRequest,
    ) {
        match self.inspect_compose(cookie, world).await {
            Ok(snapshot) => tracing::debug!(
                clean = snapshot.clean,
                expected_message_staged = snapshot.recipient == expected.message.recipient
                    && snapshot.subject == expected.message.subject
                    && snapshot.body == expected.message.body,
                credits = snapshot.credits,
                cod = snapshot.cod,
                attachment_count = snapshot.attachment_count,
                "MoogleMail send failed; compose page rechecked"
            ),
            Err(error) => {
                if error.category == MoogleMailErrorCategory::SessionExpired {
                    // 诊断 GET 也可证明凭据失效；不覆盖原投递结果，由原 session 回写撤销。
                    cookie.revoke();
                }
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "read_failed_compose",
                    safe_diagnostics::project(&error),
                    DiagnosticContext::default(),
                );
            }
        }
    }

    /// 执行一次发送并在失败后回读 compose；不自动重试远程写入。
    pub async fn send(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        self.send_with_baseline(cookie, request, None).await
    }

    /// 消费同一独占 session 在 sending fence 前取得的完整基线，写后证明保持不变。
    pub(crate) async fn send_prepared(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
        baseline: MoogleMailSentBaseline,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        self.send_with_baseline(cookie, request, Some(baseline)).await
    }

    async fn send_with_baseline(
        &self,
        cookie: &MoogleMailCookie,
        request: MoogleMailSendRequest,
        baseline: Option<MoogleMailSentBaseline>,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        let mut write_started = false;
        let outcome = self
            .send_verified(cookie, &request, baseline, &mut write_started)
            .await
            .map_err(|error| preserve_write_unknown(error, write_started));
        let failed = !matches!(outcome, Ok(MoogleMailSendOutcome::Sent(_)));
        if failed
            && !outcome
                .as_ref()
                .is_err_and(|error| error.category == MoogleMailErrorCategory::SessionExpired)
        {
            // 写入结果未知时 Sent 可能已变化，旧基线不能再作为下一次的排除集。
            self.forget_sent_baseline(request.world);
            self.diagnose_after_failure(cookie, request.world, &request).await;
        }
        outcome
    }

    /// 执行装备准备、compose 附件/COD 分步写入并证明最终发送结果。
    /// 任何一次 POST 成功但后续页面无法证明的情况都会返回 write_unknown；调用方不得把
    /// 该结果自动重试为第二次远程写入。
    async fn send_verified(
        &self,
        cookie: &MoogleMailCookie,
        request: &MoogleMailSendRequest,
        baseline: Option<MoogleMailSentBaseline>,
        write_started: &mut bool,
    ) -> Result<MoogleMailSendOutcome, MoogleMailError> {
        if !request.validate() {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Send,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let baseline = match baseline {
            Some(baseline) => baseline,
            // 独立调用仍自行准备；持久 delivery 只能使用 fence 前已完成的基线。
            None => self.prepare_sent_baseline(cookie, request.world).await?,
        };
        if baseline.world != request.world {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Sent,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        self.check_baseline_scope(cookie, &baseline)?;
        let compose = self.client.compose(request.world)?;
        let initial = self.client.get(&compose, cookie).await?;
        let parsed_initial = self.parser.parse_compose_state(&initial, request.world);
        let dirty = matches!(&parsed_initial, Ok(state) if !state.is_clean());
        let initial_parse = match parsed_initial {
            Ok(state) if state.is_clean() => Ok(state),
            Ok(_) => {
                Err(MoogleMailError::protocol_drift(MoogleMailOperation::Compose, request.world))
            }
            Err(error) => Err(error),
        };
        let mut state = match self.client.diagnose_parse(&compose, &initial, initial_parse).await {
            Ok(state) => state,
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "parse_compose_state",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );
                return Ok(MoogleMailSendOutcome::PreflightUnproven { dirty });
            }
        };

        self.prepare_equipment(cookie, request, write_started).await?;
        if *write_started {
            // Armory 写入可能使先前 compose token 失效；重新证明空草稿后才暂存任何资产。
            let refreshed = self.client.get(&compose, cookie).await?;
            let parsed =
                self.parser.parse_compose_state(&refreshed, request.world).and_then(|state| {
                    if state.is_clean() {
                        Ok(state)
                    } else {
                        Err(MoogleMailError::protocol_drift(
                            MoogleMailOperation::Compose,
                            request.world,
                        ))
                    }
                });
            state = self.client.diagnose_parse(&compose, &refreshed, parsed).await?;
        }

        if request.credits > 0 {
            let form =
                MoogleMailForm::attach_credits(state.token(), &request.message, request.credits)
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_send",
                            "build_credit_form",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        write_error(
                            request.world,
                            MoogleMailOperation::AttachAdd,
                            MoogleMailErrorCategory::Configuration,
                        )
                    })?;
            let response = self.client.post(&compose, cookie, form).await?;
            // 后续请求即使尚未发出，已暂存的资产也不能作为未写入处理。
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &[],
                        0,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachAdd))?;
        }

        for (index, equipment_id) in request.equipment_ids.iter().enumerate() {
            let form =
                MoogleMailForm::attach_equipment(state.token(), &request.message, equipment_id)
                    .map_err(|diagnostic_cause| {
                        safe_diagnostics::warn(
                            "mooglemail_send",
                            "build_equipment_form",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext {
                                world: diagnostic_world(request.world.as_str()),
                                ..Default::default()
                            },
                        );
                        write_unknown(request.world, MoogleMailOperation::AttachAdd)
                    })?;
            let response = self.client.post(&compose, cookie, form).await?;
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &request.equipment_ids[..=index],
                        0,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachAdd))?;
        }

        if request.cod > 0 {
            let form = MoogleMailForm::attach_cod(state.token(), &request.message, request.cod)
                .map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "mooglemail_send",
                        "build_cod_form",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext {
                            world: diagnostic_world(request.world.as_str()),
                            ..Default::default()
                        },
                    );
                    write_unknown(request.world, MoogleMailOperation::AttachCod)
                })?;
            let response = self.client.post(&compose, cookie, form).await?;
            *write_started = true;
            state = self
                .client
                .diagnose_parse(
                    &compose,
                    &response,
                    self.parser.prove_staged_compose(
                        &response,
                        request.world,
                        &request.message,
                        request.credits,
                        &request.equipment_ids,
                        request.cod,
                    ),
                )
                .await
                .map_err(|error| staged_proof_error(error, MoogleMailOperation::AttachCod))?;
        }

        let assets_staged =
            request.credits > 0 || request.cod > 0 || !request.equipment_ids.is_empty();
        let form =
            MoogleMailForm::send(state.token(), &request.message).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "build_send_form",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );

                if assets_staged {
                    write_unknown(request.world, MoogleMailOperation::Send)
                } else {
                    write_error(
                        request.world,
                        MoogleMailOperation::Send,
                        MoogleMailErrorCategory::Configuration,
                    )
                }
            })?;
        let response = self.client.post(&compose, cookie, form).await?;
        let (failure, confirmation) = self.parser.inspect_send_response(&response, request.world);
        if let Some(failure) =
            self.client.diagnose_parse(&compose, &response, failure).await.ok().flatten()
        {
            return Ok(match failure {
                MoogleMailSendResponseFailure::RecipientNotFound { returned_mail_id } => {
                    MoogleMailSendOutcome::RecipientNotFound { returned_mail_id }
                }
                MoogleMailSendResponseFailure::RecipientIsSender => {
                    MoogleMailSendOutcome::RecipientIsSender { assets_staged }
                }
                MoogleMailSendResponseFailure::Rejected => {
                    MoogleMailSendOutcome::Rejected { assets_staged }
                }
            });
        }
        let confirmation_hint =
            self.client.diagnose_parse(&compose, &response, confirmation).await.ok().flatten();
        let proof = async {
            let external_mail_id = self
                .verify_new_sent(request, cookie, &baseline, confirmation_hint.as_deref())
                .await?;
            // Sent 证明后必须再证明 compose 回到空表单；残留草稿意味着资产状态仍未收口。
            let state = self.inspect_compose(cookie, request.world).await?;
            if !state.is_clean() {
                return Err(write_unknown(request.world, MoogleMailOperation::Send));
            }
            Ok(MoogleMailSendOutcome::Sent(MoogleMailSendReceipt { external_mail_id }))
        }
        .await;
        match proof {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                // 即使 POST 有成功提示，缺少 Sent/compose 证明仍留存响应，不能记成功或重寄。
                let unknown = self
                    .client
                    .diagnose_parse(
                        &compose,
                        &response,
                        Err(write_unknown(request.world, MoogleMailOperation::Send)),
                    )
                    .await;
                // 保留只读证明阶段的明确登录失效及原证据，交由原 session 撤销 Cookie。
                if error.category == MoogleMailErrorCategory::SessionExpired {
                    Err(preserve_write_unknown(error, true))
                } else {
                    unknown
                }
            }
        }
    }
}

fn write_unknown(world: MoogleMailWorld, operation: MoogleMailOperation) -> MoogleMailError {
    MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
}

fn write_error(
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
    category: MoogleMailErrorCategory,
) -> MoogleMailError {
    MoogleMailError::new(operation, world, category, false)
}

fn staged_proof_error(
    mut error: MoogleMailError,
    operation: MoogleMailOperation,
) -> MoogleMailError {
    error.operation = operation;
    preserve_write_unknown(error, true)
}

/// 已发生外写后，读失败或后续写前失败都不能重新变成可安全重发；保留失效类别和原证据。
fn preserve_write_unknown(error: MoogleMailError, write_started: bool) -> MoogleMailError {
    if !write_started {
        return error;
    }
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown = MoogleMailError::after_write(error.operation, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
