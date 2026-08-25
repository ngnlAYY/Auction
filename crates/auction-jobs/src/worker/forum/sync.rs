//! 论坛主题同步 Worker。
//!
//! 该循环只编排“领取数据库快照 → 渲染 → 远端创建/查找 → 写入发布证明”，不包含拍卖状态机。
//! 远端 POST 无法证明成功时，失败码会进入人工对账状态，避免 Worker 盲目重复发帖。

use std::sync::Arc;

use auction_application::{
    ApplicationError, Clock, ForumPublisher, ForumRenderer, ForumSyncClaim, ForumSyncStore,
    RepositoryError,
};
use chrono::Duration;
use url::Url;

use super::{PersistentWorker, WorkerError};

const MAX_BATCH_SIZE: u16 = 100;
const RETRY_DELAY: Duration = Duration::minutes(1);
const MANUAL_RECONCILIATION_DELAY: Duration = Duration::days(365);

/// 论坛主题同步运行时依赖。
pub struct ForumSyncRuntime {
    pub(crate) store: Arc<dyn ForumSyncStore>,
    pub(crate) publisher: Arc<dyn ForumPublisher>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
}

impl ForumSyncRuntime {
    /// 建立论坛同步运行时，并限制批次规模、单项处理时限和公开链接 origin。
    pub fn new(
        store: Arc<dyn ForumSyncStore>,
        publisher: Arc<dyn ForumPublisher>,
        renderer: Arc<dyn ForumRenderer>,
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
        Ok(Self { store, publisher, renderer, batch_size, item_timeout, public_base_url })
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
    while processed < runtime.batch_size {
        let now = worker.clock.now();
        let lease_until = now
            .checked_add_signed(lease_duration)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)?;
        // 旧版按主题顺序逐条领取和发布；只领取即将执行的一条，避免排队任务提前耗尽租约。
        let mut claims =
            runtime.store.claim_due(worker.worker_id.clone(), now, lease_until, 1).await?;
        let Some(claim) = claims.pop() else {
            break;
        };
        processed += 1;
        if let Err(error) = process_claim(
            Arc::clone(&runtime.store),
            Arc::clone(&runtime.publisher),
            Arc::clone(&runtime.renderer),
            Arc::clone(&worker.clock),
            runtime.item_timeout,
            runtime.public_base_url.clone(),
            claim,
        )
        .await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn process_claim<C>(
    store: Arc<dyn ForumSyncStore>,
    publisher: Arc<dyn ForumPublisher>,
    renderer: Arc<dyn ForumRenderer>,
    clock: Arc<C>,
    item_timeout: std::time::Duration,
    public_base_url: Option<String>,
    claim: ForumSyncClaim,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let failure_store = Arc::clone(&store);
    let failure_clock = Arc::clone(&clock);
    let failure_claim = claim.clone();
    match tokio::time::timeout(
        item_timeout,
        process_claim_inner(store, publisher, renderer, clock, public_base_url, claim),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            record_failure(
                failure_store,
                failure_clock,
                failure_claim,
                "write_unknown",
                ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    "论坛同步单项处理超时，远端写入结果未知".to_owned(),
                )),
            )
            .await?;
            Ok(())
        }
    }
}

async fn process_claim_inner<C>(
    store: Arc<dyn ForumSyncStore>,
    publisher: Arc<dyn ForumPublisher>,
    renderer: Arc<dyn ForumRenderer>,
    clock: Arc<C>,
    public_base_url: Option<String>,
    claim: ForumSyncClaim,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let retry_claim = claim.clone();
    let snapshot = match store.load_snapshot(claim.clone()).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            record_failure(store, clock, retry_claim, "snapshot_failed", error).await?;
            return Ok(());
        }
    };
    let mut snapshot = snapshot;
    snapshot.public_url =
        match resolve_public_url(public_base_url.as_deref(), snapshot.public_url.as_deref()) {
            Ok(public_url) => public_url,
            Err(error) => {
                record_failure(store, clock, retry_claim, "configuration", error).await?;
                return Ok(());
            }
        };
    let world = snapshot.world;
    let rendered = match renderer.render(snapshot) {
        Ok(rendered) => rendered,
        Err(error) => {
            record_failure(
                store,
                clock,
                retry_claim,
                match error {
                    auction_application::ForumRenderError::InvalidSnapshot => "invalid_snapshot",
                    auction_application::ForumRenderError::BodyTooLarge => "body_too_large",
                },
                ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    error.to_string(),
                )),
            )
            .await?;
            return Ok(());
        }
    };
    let published = match publisher.create_topic(world, rendered.clone()).await {
        Ok(published) => published,
        Err(error) => {
            let failure_code = forum_failure_code(&error);
            record_failure(store, clock, retry_claim, failure_code, error).await?;
            return Ok(());
        }
    };
    if let Err(error) = store.publish(claim.clone(), rendered, published, clock.now()).await {
        record_failure(store, clock, claim, "publish_proof_failed", error).await?;
    }
    Ok(())
}

async fn record_failure<C>(
    store: Arc<dyn ForumSyncStore>,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    failure_code: &str,
    error: ApplicationError,
) -> Result<(), ApplicationError>
where
    C: Clock + 'static,
{
    let now = clock.now();
    let delay = if is_manual_failure(&error, failure_code) {
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
    store.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await
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
        ApplicationError::ForumRemote(remote) if remote.write_started()
    ) || matches!(
        failure_code,
        "invalid_snapshot"
            | "body_too_large"
            | "configuration"
            | "credential_unavailable"
            | "protocol_drift"
            | "write_unknown"
            | "publish_proof_failed"
    )
}

fn bounded_detail(value: String) -> String {
    let detail: String = value.chars().take(512).collect();
    if detail.is_empty() { "论坛同步失败".to_owned() } else { detail }
}

fn valid_public_base_url(value: &str) -> bool {
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    (parsed.scheme() == "https"
        || (parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path().is_empty()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && value.trim() == value
}

fn resolve_public_url(
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
