//! 论坛主题同步 Worker；只编排领取数据库快照 → 渲染 → 远端创建/查找 → 写入发布证明，
//! 远端 POST 无法证明成功时进入人工对账而不重复发帖。

use std::sync::{Arc, OnceLock};

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{ApplicationError, Clock, RepositoryError};
use auction_application::forum::{
    ForumCredentialAvailability, ForumPostWriter, ForumPublishedThread, ForumPublisher,
    ForumRenderedThread, ForumRenderer, ForumSyncClaim, ForumSyncStore, ForumTopicPublication,
    ForumTopicReader, ForumWriteProgress,
};
use auction_application::public::PublicEquipmentLinkResolver;
use chrono::Duration;
use url::Url;

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError};

mod layout;
use layout::{FixedLayoutRequest, ensure_fixed_layout};

const MAX_BATCH_SIZE: u16 = 100;
const RETRY_DELAY: Duration = Duration::minutes(1);
const MANUAL_RECONCILIATION_DELAY: Duration = Duration::days(365);

/// 论坛主题同步运行时依赖。
pub struct ForumSyncPorts {
    pub store: Arc<dyn ForumSyncStore>,
    pub credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub publisher: Arc<dyn ForumPublisher>,
    pub reader: Arc<dyn ForumTopicReader>,
    pub post_writer: Arc<dyn ForumPostWriter>,
    pub renderer: Arc<dyn ForumRenderer>,
    pub equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
}

pub struct ForumSyncRuntime {
    pub(crate) store: Arc<dyn ForumSyncStore>,
    pub(crate) credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub(crate) publisher: Arc<dyn ForumPublisher>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) post_writer: Arc<dyn ForumPostWriter>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
    pub(crate) verify_post_body: bool,
    pub(crate) startup_recovery_before: OnceLock<chrono::DateTime<chrono::Utc>>,
}

impl ForumSyncRuntime {
    /// 建立论坛同步运行时，并限制批次规模、单项处理时限和公开链接 origin。
    pub fn new(
        ports: ForumSyncPorts,
        batch_size: u16,
        item_timeout: std::time::Duration,
        public_base_url: Option<String>,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0
            || batch_size > MAX_BATCH_SIZE
            || item_timeout.is_zero()
            || public_base_url.as_deref().is_some_and(|value| !valid_public_base_url(value))
        {
            return Err(WorkerError::InvalidForumSyncConfiguration);
        }
        let ForumSyncPorts {
            store,
            credential_availability,
            publisher,
            reader,
            post_writer,
            renderer,
            equipment_links,
        } = ports;
        Ok(Self {
            store,
            credential_availability,
            publisher,
            reader,
            post_writer,
            renderer,
            equipment_links,
            batch_size,
            item_timeout,
            public_base_url,
            verify_post_body: false,
            startup_recovery_before: OnceLock::new(),
        })
    }

    /// 配置固定帖原始正文校验；默认关闭，主题身份、回执及未知写入禁止重发的门禁始终保留。
    pub fn with_post_body_verification(mut self, enabled: bool) -> Self {
        self.verify_post_body = enabled;
        self
    }
}

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumSyncRuntime,
    lease_duration: std::time::Duration,
    hot: Option<auction_application::admin::WorkerForumHotConfig>,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let lease_duration = Duration::from_std(lease_duration)
        .map_err(|_| WorkerError::InvalidForumSyncConfiguration)
        .inspect_err(|error| {
            diagnostics::unpersisted_error(
                "forum_sync",
                "convert_lease_duration",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
        })?;
    // 热快照只替换批次与单项预算；与启动租约冲突时退回启动值，不能让 claim 越过租约。
    let (batch_size, item_timeout) = hot
        .filter(|hot| {
            let item = std::time::Duration::from_secs(hot.item_timeout_seconds);
            hot.batch_size > 0
                && hot.batch_size <= MAX_BATCH_SIZE
                && !item.is_zero()
                && lease_duration.to_std().is_ok_and(|lease| item < lease)
        })
        .map(|hot| (hot.batch_size, std::time::Duration::from_secs(hot.item_timeout_seconds)))
        .unwrap_or((runtime.batch_size, runtime.item_timeout));
    let mut processed = 0_u16;
    let mut first_error = None;
    while processed < batch_size && !worker.control.is_draining() {
        let available_worlds =
            [auction_domain::SettlementWorld::Main, auction_domain::SettlementWorld::Isekai]
                .into_iter()
                .filter(|world| {
                    runtime.credential_availability.has_available_forum_credential(*world)
                })
                .collect::<Vec<_>>();
        if available_worlds.is_empty() {
            diagnostics::unpersisted_error(
                "forum_sync",
                "require_credential",
                SafeErrorKind::CAPABILITY_UNAVAILABLE,
                DiagnosticContext::default(),
            );
            first_error.get_or_insert(WorkerError::ForumTopicProcessingFailed);
        }
        // 领取与远端处理共用领取前截止，数据库等待不能重新获得完整处理预算。
        let item_deadline = tokio::time::Instant::now()
            .checked_add(item_timeout)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "check_item_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let now = worker.clock.now();
        let recovery_before = *runtime.startup_recovery_before.get_or_init(|| now);
        let startup_recovery_before = (!available_worlds.is_empty()).then_some(recovery_before);
        let lease_until = now
            .checked_add_signed(lease_duration)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "check_lease_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        // 只领取即将执行的一条；空世界集合仍初始化 live 投影，但不占租约或发起外呼。
        let claims = worker
            .write_lifecycle
            .background(tokio::time::timeout_at(
                item_deadline,
                runtime.store.claim_due(
                    worker.worker_id.clone(),
                    now,
                    lease_until,
                    startup_recovery_before,
                    available_worlds,
                    1,
                ),
            ))
            .await;
        let Some(claims) = claims else {
            break;
        };
        let mut claims = claims
            .map_err(|timeout| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "claim_topic_timeout",
                    diagnostics::timeout_kind(&timeout),
                    DiagnosticContext::default(),
                );
                let _elapsed = timeout;
                WorkerError::ForumTopicProcessingFailed
            })?
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "claim_topic",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let Some(claim) = claims.pop() else {
            break;
        };
        // 领取回执跨过 drain 时尚未外呼；保留原 claim，不能在停机阶段开始发布主题。
        if worker.control.is_draining() {
            break;
        }
        processed += 1;
        let diagnostic_auction_id = claim.auction_id;
        if let Err(error) = process_claim(
            runtime,
            Arc::clone(&worker.clock),
            claim,
            item_deadline,
            &worker.write_lifecycle,
        )
        .await
        .inspect_err(|error| {
            if !matches!(error, WorkerError::ForumTopicProcessingFailed) {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "process_claim",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        auction_id: diagnostic_id(&diagnostic_auction_id),
                        ..Default::default()
                    },
                );
            }
        }) && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[tracing::instrument(

    skip_all,

    fields(

        auction_id = %claim.auction_id,

    )

)]
async fn process_claim<C: Clock + 'static>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    item_deadline: tokio::time::Instant,
    lifecycle: &auction_application::operations::ExternalWriteLifecycle,
) -> Result<(), WorkerError> {
    let activity = lifecycle.activity(auction_application::operations::CriticalWriteKind::Forum);
    let progress = ForumWriteProgress::with_activity(activity.clone());
    activity
        .run(process_claim_attempt(runtime, clock, claim, item_deadline, &progress))
        .await
        .unwrap_or(Ok(()))
}

async fn process_claim_attempt<C>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    item_deadline: tokio::time::Instant,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let store = Arc::clone(&runtime.store);
    let failure_store = Arc::clone(&store);
    let failure_clock = Arc::clone(&clock);
    let failure_claim = claim.clone();
    let reconciliation_only = requires_reconciliation(&claim);
    let result = if tokio::time::Instant::now() >= item_deadline {
        // 领取响应已到截止时，不首次 poll 内层任务；仍记录已知未开始外写的安全失败。
        None
    } else {
        tokio::time::timeout_at(
            item_deadline,
            process_claim_inner(runtime, clock, claim, write_progress),
        )
        .await
        .ok()
    };
    match result {
        Some(result) => result,
        None => {
            // 只有真实 POST 边界或历史未知结果才保留只读门禁；纯预读/渲染超时可普通重试。
            let failure_code = if write_progress.has_unconfirmed_write() {
                "write_unknown"
            } else {
                "topic_processing_timeout"
            };
            record_failure(
                write_progress,
                failure_store,
                failure_clock,
                failure_claim,
                scoped_failure_code(reconciliation_only, failure_code),
                ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
                    "论坛同步单项处理超时，按实际写入进度保留重试边界".to_owned(),
                )),
            )
            .await?;
            Err(WorkerError::ForumTopicProcessingFailed)
        }
    }
}

async fn process_claim_inner<C>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let store = Arc::clone(&runtime.store);
    let publisher = Arc::clone(&runtime.publisher);
    let reader = Arc::clone(&runtime.reader);
    let post_writer = Arc::clone(&runtime.post_writer);
    let renderer = Arc::clone(&runtime.renderer);
    let equipment_links = Arc::clone(&runtime.equipment_links);
    let reconciliation_only = requires_reconciliation(&claim);
    let retry_authorized = claim.previous_failure_code.as_deref() == Some("write_retry_authorized");
    let retry_claim = claim.clone();
    let snapshot = match store.load_snapshot(claim.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            record_failure(
                write_progress,
                store,
                clock,
                retry_claim,
                scoped_failure_code(reconciliation_only, "snapshot_failed"),
                error,
            )
            .await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    let mut snapshot = snapshot;
    snapshot.public_url = match resolve_public_url(
        runtime.public_base_url.as_deref(),
        snapshot.public_url.as_deref(),
    ) {
        Ok(public_url) => public_url,
        Err(error) => {
            record_failure(
                write_progress,
                store,
                clock,
                retry_claim,
                scoped_failure_code(reconciliation_only, "configuration"),
                error,
            )
            .await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    let auction_id = snapshot.auction_id;
    let world = snapshot.world;
    let link_targets =
        snapshot.lots.iter().map(|lot| lot.equipment_link_target.clone()).collect::<Vec<_>>();
    let equipment_links = match equipment_links.resolve_public_equipment_links(&link_targets).await
    {
        Ok(links) => links,
        Err(error) => {
            record_failure(
                write_progress,
                store,
                clock,
                retry_claim,
                scoped_failure_code(reconciliation_only, "equipment_links_failed"),
                error,
            )
            .await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    let rendered = match renderer.render(snapshot, equipment_links) {
        Ok(rendered) => rendered,
        Err(error) => {
            record_failure(
                write_progress,
                store,
                clock,
                retry_claim,
                scoped_failure_code(
                    reconciliation_only,
                    match error {
                        auction_application::forum::ForumRenderError::InvalidSnapshot => {
                            "invalid_snapshot"
                        }
                        auction_application::forum::ForumRenderError::BodyTooLarge => {
                            "body_too_large"
                        }
                    },
                ),
                ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
                    error.to_string(),
                )),
            )
            .await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    let catalog_page_count = rendered.catalog_continuation.len() + 1;
    let system_log = match renderer.render_system_log(
        auction_id,
        catalog_page_count,
        catalog_page_count + 1,
        Vec::new(),
    ) {
        Ok(rendered) => rendered,
        Err(error) => {
            record_failure(
                write_progress,
                store,
                clock,
                retry_claim,
                scoped_failure_code(
                    reconciliation_only,
                    match error {
                        auction_application::forum::ForumRenderError::InvalidSnapshot => {
                            "invalid_snapshot"
                        }
                        auction_application::forum::ForumRenderError::BodyTooLarge => {
                            "body_too_large"
                        }
                    },
                ),
                ApplicationError::Repository(RepositoryError::Failed(error.to_string())),
            )
            .await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    let published = if reconciliation_only {
        match find_reconciliation_topic(publisher.as_ref(), world, &rendered).await {
            Ok(Some(published)) => ForumTopicPublication::Existing(published),
            Ok(None) if retry_authorized => {
                match publisher.create_topic(world, rendered.clone(), write_progress.clone()).await
                {
                    Ok(published) => published,
                    Err(error) => {
                        // 已授权重试也只能在远端尚未开始写入时自动再次提交；一旦结果未知，
                        // 下一轮必须重新回到精确标题只读对账。
                        let failure_code = if matches!(
                            &error,
                            ApplicationError::ForumRemote(remote) if remote.write_started()
                        ) {
                            "write_unknown"
                        } else {
                            forum_failure_code(&error)
                        };
                        record_failure(
                            write_progress,
                            store,
                            clock,
                            retry_claim,
                            failure_code.to_owned(),
                            error,
                        )
                        .await?;
                        return Err(WorkerError::ForumTopicProcessingFailed);
                    }
                }
            }
            Ok(None) => {
                record_failure(
                    write_progress,
                    store,
                    clock,
                    retry_claim,
                    "write_authorization_required".to_owned(),
                    ApplicationError::Repository(RepositoryError::Failed(
                        "精确标题只读对账未发现主题，必须人工授权后才能重新发帖".to_owned(),
                    )),
                )
                .await?;
                return Err(WorkerError::ForumTopicProcessingFailed);
            }
            Err(error) => {
                let failure_code = scoped_failure_code(true, forum_failure_code(&error));
                record_failure(write_progress, store, clock, retry_claim, failure_code, error)
                    .await?;
                return Err(WorkerError::ForumTopicProcessingFailed);
            }
        }
    } else {
        match publisher.create_topic(world, rendered.clone(), write_progress.clone()).await {
            Ok(published) => published,
            Err(error) => {
                // 只要远端写入已经开始，后续状态一律先进入精确标题只读对账。
                let failure_code = if matches!(
                    &error,
                    ApplicationError::ForumRemote(remote) if remote.write_started()
                ) {
                    "write_unknown"
                } else {
                    forum_failure_code(&error)
                };
                record_failure(
                    write_progress,
                    store,
                    clock,
                    retry_claim,
                    failure_code.to_owned(),
                    error,
                )
                .await?;
                return Err(WorkerError::ForumTopicProcessingFailed);
            }
        }
    };
    let (published, newly_created) = match published {
        ForumTopicPublication::Created(published) => (published, true),
        ForumTopicPublication::Existing(published) => (published, false),
    };
    // create_topic 也可能只找到旧主题；只有本轮真实创建或人工授权才允许补写固定楼层。
    let allow_layout_writes = newly_created || retry_authorized;
    let legacy_thread_id = claim.existing_thread_id.clone();
    let layout = match ensure_fixed_layout(
        Arc::clone(&reader),
        Arc::clone(&post_writer),
        FixedLayoutRequest {
            world,
            auction_id,
            published,
            catalog: rendered.clone(),
            system_log: system_log.clone(),
            allow_layout_writes,
            allow_content_update: retry_authorized,
            catalog_written: newly_created,
            verify_post_body: runtime.verify_post_body,
            legacy_thread_id,
            write_progress: write_progress.clone(),
        },
    )
    .await
    {
        Ok(layout) => layout,
        Err(error) => {
            // 已取得主题创建/查找证明；后续 GET 或固定楼层失败不能退回可自动发新主题的路径。
            let code = forum_failure_code(&error);
            // 人工补写授权使用已冻结的精确错误码，领取门禁会阻止自动重试。
            let failure_code = if code == "write_authorization_required" {
                code.to_owned()
            } else {
                scoped_failure_code(true, code)
            };
            record_failure(write_progress, store, clock, retry_claim, failure_code, error).await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    if let Err(error) =
        store.publish(claim.clone(), rendered, system_log, layout, clock.now()).await
    {
        record_failure(
            write_progress,
            store,
            clock,
            claim,
            "publish_proof_failed".to_owned(),
            error,
        )
        .await?;
        return Err(WorkerError::ForumTopicProcessingFailed);
    }
    write_progress.confirm_write();
    Ok(())
}

async fn find_reconciliation_topic(
    publisher: &dyn ForumPublisher,
    world: auction_domain::SettlementWorld,
    rendered: &ForumRenderedThread,
) -> Result<Option<ForumPublishedThread>, ApplicationError> {
    // 一次版面读取同时检查当前及全部旧标题，不能按候选数放大读取与限速预算。
    let titles =
        std::iter::once(&rendered.title).chain(&rendered.reconciliation_titles).cloned().collect();
    publisher.find_topic_by_titles(world, titles).await
}

async fn record_failure<C>(
    write_progress: &ForumWriteProgress,
    store: Arc<dyn ForumSyncStore>,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    failure_code: String,
    error: ApplicationError,
) -> Result<(), ApplicationError>
where
    C: Clock + 'static,
{
    if write_progress.is_cancelled() {
        return Ok(());
    }
    let now = clock.now();
    let unscoped_code = failure_code.strip_prefix("reconcile_").unwrap_or(&failure_code);
    let delay = if matches!(unscoped_code, "write_unknown" | "publish_proof_failed") {
        // 带 reconcile_ 的未知结果同样只能 GET 对账，不能误用一年退避使已成功的编辑永久搁置。
        RETRY_DELAY
    } else if is_manual_failure(&error, &failure_code) {
        MANUAL_RECONCILIATION_DELAY
    } else {
        RETRY_DELAY
    };
    let Some(retry_at) = now.checked_add_signed(delay) else {
        // 不把人工对账或重试时间折回当前时刻，避免未知远端结果被自动再次提交。
        return Err(ApplicationError::Repository(RepositoryError::Failed(
            "论坛同步重试时间溢出".to_owned(),
        )));
    };
    let safe_detail = bounded_detail(error.to_string());
    store.fail(claim, retry_at, failure_code, safe_detail).await?;
    write_progress.confirm_write();
    Ok(())
}

fn forum_failure_code(error: &ApplicationError) -> &'static str {
    match error {
        ApplicationError::ForumRemote(remote) => remote.code(),
        ApplicationError::CapabilityUnavailable(_) => "capability_unavailable",
        _ => "remote_failed",
    }
}

fn is_manual_failure(error: &ApplicationError, failure_code: &str) -> bool {
    matches!(
        error,
        ApplicationError::ForumRemote(remote) if remote.write_started() || !remote.retryable()
    ) || matches!(
        failure_code.strip_prefix("reconcile_").unwrap_or(failure_code),
        "invalid_snapshot"
            | "body_too_large"
            | "equipment_links_failed"
            | "configuration"
            | "credential_unavailable"
            | "protocol_drift"
            | "fixed_layout_conflict"
            | "write_authorization_required"
            | "capability_unavailable"
    )
}

fn requires_reconciliation(claim: &ForumSyncClaim) -> bool {
    claim.previous_failure_code.as_deref().is_some_and(|code| {
        matches!(code, "write_unknown" | "publish_proof_failed" | "write_retry_authorized")
            || code.starts_with("reconcile_")
    })
}

fn scoped_failure_code(reconciliation_only: bool, failure_code: &str) -> String {
    if reconciliation_only && !failure_code.starts_with("reconcile_") {
        format!("reconcile_{failure_code}")
    } else {
        failure_code.to_owned()
    }
}

fn bounded_detail(value: String) -> String {
    let detail: String = value.chars().take(512).collect();
    if detail.is_empty() { "论坛同步失败".to_owned() } else { detail }
}

pub(super) fn valid_public_base_url(value: &str) -> bool {
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    (parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && (parsed.path().is_empty() || parsed.path() == "/")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && value.trim() == value
}

pub(super) fn resolve_public_url(
    base_url: Option<&str>,
    reference: Option<&str>,
) -> Result<Option<String>, ApplicationError> {
    let Some(base_url) = base_url else {
        return Ok(None);
    };
    let Some(reference) = reference.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let base = Url::parse(base_url).map_err(|error| {
        diagnostics::warn(
            "forum_sync",
            "parse_public_base_url",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );

        ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
            "论坛公开地址配置无效".to_owned(),
        ))
    })?;
    if !reference.starts_with('/')
        || reference.starts_with("//")
        || reference.contains('#')
        || reference.chars().any(char::is_control)
    {
        return Err(ApplicationError::Repository(
            auction_application::core::RepositoryError::Failed("论坛公开地址引用无效".to_owned()),
        ));
    }
    let resolved = base.join(reference).map_err(|error| {
        diagnostics::warn(
            "forum_sync",
            "join_public_url",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );

        ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
            "论坛公开地址引用无效".to_owned(),
        ))
    })?;
    if resolved.scheme() != base.scheme() || resolved.host() != base.host() {
        return Err(ApplicationError::Repository(
            auction_application::core::RepositoryError::Failed("论坛公开地址引用无效".to_owned()),
        ));
    }
    Ok(Some(resolved.to_string()))
}
