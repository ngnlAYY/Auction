//! 论坛主题同步 Worker。
//!
//! 该循环只编排“领取数据库快照 → 渲染 → 远端创建/查找 → 写入发布证明”，不包含拍卖状态机。
//! 远端 POST 无法证明成功时，失败码会进入人工对账状态，避免 Worker 盲目重复发帖。

use std::sync::{Arc, OnceLock};

use auction_application::{
    ApplicationError, Clock, ForumCredentialAvailability, ForumPostWriter, ForumPublishedThread,
    ForumPublisher, ForumRenderedThread, ForumRenderer, ForumSyncClaim, ForumSyncStore,
    ForumTopicPublication, ForumTopicReader, PublicEquipmentLinkResolver, RepositoryError,
};
use chrono::Duration;
use url::Url;

use super::{PersistentWorker, WorkerError};

#[path = "sync/layout.rs"]
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

/// 论坛主题同步运行时依赖。
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
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let lease_duration = Duration::from_std(lease_duration)
        .map_err(|_| WorkerError::InvalidForumSyncConfiguration)?;
    let mut processed = 0_u16;
    let mut first_error = None;
    while processed < runtime.batch_size && !worker.control.is_draining() {
        let now = worker.clock.now();
        let recovery_before = *runtime.startup_recovery_before.get_or_init(|| now);
        let startup_recovery_before = runtime
            .credential_availability
            .has_available_forum_credential()
            .then_some(recovery_before);
        let lease_until = now
            .checked_add_signed(lease_duration)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)?;
        // 旧版按主题顺序逐条领取和发布；只领取即将执行的一条，避免排队任务提前耗尽租约。
        let mut claims = runtime
            .store
            .claim_due(worker.worker_id.clone(), now, lease_until, startup_recovery_before, 1)
            .await?;
        let Some(claim) = claims.pop() else {
            break;
        };
        processed += 1;
        if let Err(error) = process_claim(runtime, Arc::clone(&worker.clock), claim).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn process_claim<C>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let store = Arc::clone(&runtime.store);
    let failure_store = Arc::clone(&store);
    let failure_clock = Arc::clone(&clock);
    let failure_claim = claim.clone();
    let reconciliation_only = requires_reconciliation(&claim);
    match tokio::time::timeout(runtime.item_timeout, process_claim_inner(runtime, clock, claim))
        .await
    {
        Ok(result) => result,
        Err(_) => {
            record_failure(
                failure_store,
                failure_clock,
                failure_claim,
                scoped_failure_code(reconciliation_only, "write_unknown"),
                ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    "论坛同步单项处理超时，远端写入结果未知".to_owned(),
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
                store,
                clock,
                retry_claim,
                scoped_failure_code(
                    reconciliation_only,
                    match error {
                        auction_application::ForumRenderError::InvalidSnapshot => {
                            "invalid_snapshot"
                        }
                        auction_application::ForumRenderError::BodyTooLarge => "body_too_large",
                    },
                ),
                ApplicationError::Repository(auction_application::RepositoryError::Failed(
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
                store,
                clock,
                retry_claim,
                scoped_failure_code(
                    reconciliation_only,
                    match error {
                        auction_application::ForumRenderError::InvalidSnapshot => {
                            "invalid_snapshot"
                        }
                        auction_application::ForumRenderError::BodyTooLarge => "body_too_large",
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
                match publisher.create_topic(world, rendered.clone()).await {
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
                        record_failure(store, clock, retry_claim, failure_code.to_owned(), error)
                            .await?;
                        return Err(WorkerError::ForumTopicProcessingFailed);
                    }
                }
            }
            Ok(None) => {
                record_failure(
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
                record_failure(store, clock, retry_claim, failure_code, error).await?;
                return Err(WorkerError::ForumTopicProcessingFailed);
            }
        }
    } else {
        match publisher.create_topic(world, rendered.clone()).await {
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
                record_failure(store, clock, retry_claim, failure_code.to_owned(), error).await?;
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
            record_failure(store, clock, retry_claim, failure_code, error).await?;
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
    };
    if let Err(error) =
        store.publish(claim.clone(), rendered, system_log, layout, clock.now()).await
    {
        record_failure(store, clock, claim, "publish_proof_failed".to_owned(), error).await?;
        return Err(WorkerError::ForumTopicProcessingFailed);
    }
    Ok(())
}

async fn find_reconciliation_topic(
    publisher: &dyn ForumPublisher,
    world: auction_domain::SettlementWorld,
    rendered: &ForumRenderedThread,
) -> Result<Option<ForumPublishedThread>, ApplicationError> {
    if let Some(published) = publisher.find_topic_by_title(world, rendered.title.clone()).await? {
        return Ok(Some(published));
    }
    let Some(legacy_title) = rendered.legacy_title.as_deref() else {
        return Ok(None);
    };
    if legacy_title == rendered.title {
        return Ok(None);
    }
    publisher.find_topic_by_title(world, legacy_title.to_owned()).await
}

async fn record_failure<C>(
    store: Arc<dyn ForumSyncStore>,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    failure_code: String,
    error: ApplicationError,
) -> Result<(), ApplicationError>
where
    C: Clock + 'static,
{
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
    store.fail(claim, retry_at, failure_code, safe_detail).await
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
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
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
    let base = Url::parse(base_url).map_err(|_| {
        ApplicationError::Repository(auction_application::RepositoryError::Failed(
            "论坛公开地址配置无效".to_owned(),
        ))
    })?;
    if !reference.starts_with('/')
        || reference.starts_with("//")
        || reference.contains('#')
        || reference.chars().any(char::is_control)
    {
        return Err(ApplicationError::Repository(auction_application::RepositoryError::Failed(
            "论坛公开地址引用无效".to_owned(),
        )));
    }
    let resolved = base.join(reference).map_err(|_| {
        ApplicationError::Repository(auction_application::RepositoryError::Failed(
            "论坛公开地址引用无效".to_owned(),
        ))
    })?;
    if resolved.scheme() != base.scheme() || resolved.host() != base.host() {
        return Err(ApplicationError::Repository(auction_application::RepositoryError::Failed(
            "论坛公开地址引用无效".to_owned(),
        )));
    }
    Ok(Some(resolved.to_string()))
}
