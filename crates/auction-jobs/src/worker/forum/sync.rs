//! 论坛主题同步 Worker。
//!
//! 该循环只编排“领取数据库快照 → 渲染 → 远端创建/查找 → 写入发布证明”，不包含拍卖状态机。
//! 远端 POST 无法证明成功时，失败码会进入人工对账状态，避免 Worker 盲目重复发帖。

use std::sync::{Arc, OnceLock};

use auction_application::{
    ApplicationError, Clock, ForumCredentialAvailability, ForumPostWriter, ForumPublishedLayout,
    ForumPublishedThread, ForumPublisher, ForumRemoteError, ForumRenderedPost, ForumRenderedThread,
    ForumRenderer, ForumSyncClaim, ForumSyncStore, ForumTopicMetadata, ForumTopicPublication,
    ForumTopicReader, PublicEquipmentLinkResolver, RepositoryError, forum_catalog_marker,
    forum_post_bodies_match, forum_system_log_marker,
};
use chrono::Duration;
use url::Url;

use super::{PersistentWorker, WorkerError};

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
    let system_log = match renderer.render_system_log(auction_id, 2, Vec::new()) {
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

struct FixedLayoutRequest {
    world: auction_domain::SettlementWorld,
    auction_id: auction_domain::AuctionId,
    published: ForumPublishedThread,
    catalog: ForumRenderedThread,
    system_log: ForumRenderedPost,
    allow_layout_writes: bool,
    allow_content_update: bool,
    catalog_written: bool,
    verify_post_body: bool,
    legacy_thread_id: Option<String>,
}

async fn ensure_fixed_layout(
    reader: Arc<dyn ForumTopicReader>,
    writer: Arc<dyn ForumPostWriter>,
    request: FixedLayoutRequest,
) -> Result<ForumPublishedLayout, ApplicationError> {
    let FixedLayoutRequest {
        world,
        auction_id,
        published,
        catalog,
        system_log,
        allow_layout_writes,
        allow_content_update,
        mut catalog_written,
        verify_post_body,
        legacy_thread_id,
    } = request;
    let mut preview = reader.fetch_topic(world, published.thread_id.clone()).await?;
    let catalog_marker = forum_catalog_marker(auction_id);
    let system_log_marker = forum_system_log_marker(auction_id);
    if !preview.catalog_post_text.contains(&catalog_marker) {
        let belongs_to_other_auction = preview.catalog_post_text.contains("auction-catalog-v1:");
        let legacy_proof_matches =
            legacy_thread_id.as_deref() == Some(published.thread_id.as_str());
        if belongs_to_other_auction || !legacy_proof_matches {
            return Err(fixed_layout_error("fixed_layout_conflict", false));
        }
        // 未知写入恢复只能读取；历史布局升级也不能借此绕过人工授权再次 POST。
        if !allow_layout_writes {
            return Err(fixed_layout_error("write_authorization_required", false));
        }
        let proof = writer
            .edit_post(
                world,
                published.thread_id.clone(),
                preview.catalog_post_id.clone(),
                Some(ForumTopicMetadata {
                    title: Some(catalog.title.clone()),
                    description: Some(catalog.description.clone()),
                }),
                ForumRenderedPost { body: catalog.body.clone() },
            )
            .await?;
        if proof.thread_id != published.thread_id || proof.post_id != preview.catalog_post_id {
            return Err(fixed_layout_error("write_unknown", true));
        }
        preview = reader.fetch_topic(world, published.thread_id.clone()).await?;
        if preview.catalog_post_id != proof.post_id
            || !preview.catalog_post_text.contains(&catalog_marker)
        {
            return Err(fixed_layout_error("write_unknown", true));
        }
        catalog_written = true;
    }
    let catalog_post_id = preview.catalog_post_id.clone();
    let (system_log_post_id, system_log_written) = if let Some(second) = preview.posts.first() {
        if !second.post.text.contains(&system_log_marker) {
            return Err(fixed_layout_error("fixed_layout_conflict", false));
        }
        // 即使目录是本轮新建，现存 #2 也不是本轮回复回执；不能把旧日志记成本轮目标。
        if !verify_post_body && !allow_content_update {
            return Err(fixed_layout_error("write_authorization_required", false));
        }
        (second.post.id.clone(), false)
    } else {
        if !allow_layout_writes {
            return Err(fixed_layout_error("write_authorization_required", false));
        }
        let proof =
            writer.reply_topic(world, published.thread_id.clone(), system_log.clone()).await?;
        if proof.thread_id != published.thread_id {
            return Err(fixed_layout_error("write_unknown", true));
        }
        let confirmed = reader.fetch_topic(world, published.thread_id.clone()).await?;
        let Some(second) = confirmed.posts.first() else {
            return Err(fixed_layout_error("write_unknown", true));
        };
        if confirmed.catalog_post_id != catalog_post_id
            || second.post.id != proof.post_id
            || !second.post.text.contains(&system_log_marker)
        {
            return Err(fixed_layout_error("fixed_layout_conflict", false));
        }
        (proof.post_id, true)
    };

    if !verify_post_body && !allow_layout_writes {
        // 现存帖子 ID 不能证明未知 POST 已写入哪一版；关闭正文校验后等待人工授权，
        // 不能把恢复期间新生成、尚未发送的镜像当作已同步正文持久化。
        return Err(fixed_layout_error("write_authorization_required", false));
    }
    // 关闭正文校验时，新发布/授权编辑以精确回执和布局确认身份，不声明正文逐字一致。
    // 未知结果不能借关闭校验伪造完成；只有显式授权才允许更新已经找到的固定帖。
    let policy = FixedPostPolicy {
        verify_body: verify_post_body,
        allow_update: allow_content_update,
        write_confirmed: catalog_written,
    };
    reconcile_or_update_fixed_post(
        writer.as_ref(),
        world,
        &published.thread_id,
        &catalog_post_id,
        Some(ForumTopicMetadata {
            title: Some(catalog.title.clone()),
            description: Some(catalog.description.clone()),
        }),
        ForumRenderedPost { body: catalog.body },
        policy,
    )
    .await?;
    reconcile_or_update_fixed_post(
        writer.as_ref(),
        world,
        &published.thread_id,
        &system_log_post_id,
        None,
        system_log,
        FixedPostPolicy { write_confirmed: system_log_written, ..policy },
    )
    .await?;
    Ok(ForumPublishedLayout { thread: published, catalog_post_id, system_log_post_id })
}

#[derive(Clone, Copy)]
struct FixedPostPolicy {
    verify_body: bool,
    allow_update: bool,
    /// 仅指本轮目标正文的创建/编辑/回复回执，不是历史帖子身份。
    write_confirmed: bool,
}

/// 已有远端布局证明后，按配置核对正文；任何内容更新仍须本轮人工授权。
async fn reconcile_or_update_fixed_post(
    writer: &dyn ForumPostWriter,
    world: auction_domain::SettlementWorld,
    thread_id: &str,
    post_id: &str,
    metadata: Option<ForumTopicMetadata>,
    expected: ForumRenderedPost,
    policy: FixedPostPolicy,
) -> Result<(), ApplicationError> {
    if policy.verify_body {
        let observed =
            writer.read_post_body(world, thread_id.to_owned(), post_id.to_owned()).await?;
        if forum_post_bodies_match(&observed.body, &expected.body) {
            return Ok(());
        }
        if policy.write_confirmed {
            // 本轮已经写过，正文证明失败不能借尚在作用域内的授权再次 POST。
            return Err(fixed_layout_error("write_unknown", true));
        }
    } else if policy.write_confirmed {
        // 本轮目标已有回执，不重复编辑刚创建、升级或补齐的固定帖。
        return Ok(());
    }
    if !policy.allow_update {
        return Err(fixed_layout_error("write_authorization_required", false));
    }
    let proof = writer
        .edit_post(world, thread_id.to_owned(), post_id.to_owned(), metadata, expected.clone())
        .await?;
    if proof.thread_id != thread_id || proof.post_id != post_id {
        return Err(fixed_layout_error("write_unknown", true));
    }
    if policy.verify_body {
        let confirmed =
            writer.read_post_body(world, thread_id.to_owned(), post_id.to_owned()).await?;
        if !forum_post_bodies_match(&confirmed.body, &expected.body) {
            return Err(fixed_layout_error("write_unknown", true));
        }
    }
    Ok(())
}

fn fixed_layout_error(code: &'static str, write_started: bool) -> ApplicationError {
    ApplicationError::ForumRemote(ForumRemoteError::new(code, false, write_started))
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
