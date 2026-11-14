//! 单条主题的远端编排：快照渲染、创建/精确标题对账、固定楼层与发布证明。

use std::sync::Arc;

use auction_application::core::{ApplicationError, Clock, RepositoryError};
use auction_application::forum::{
    ForumPublishedThread, ForumPublisher, ForumRenderError, ForumRenderedThread, ForumSyncClaim,
    ForumTopicPublication, ForumWriteProgress,
};

use super::failure::{
    create_failure_code, forum_failure_code, record_failure, render_failure_code,
    requires_reconciliation, scoped_failure_code,
};
use super::layout::{FixedLayoutRequest, ensure_fixed_layout};
use super::links::resolve_public_url;
use super::runtime::ForumSyncRuntime;
use crate::worker::runtime::WorkerError;

/// 单条主题的失败处置：持久失败码与原因。外层只记录一次失败，失败码字符串逐字保持原值。
struct ClaimFailure {
    failure_code: String,
    error: ApplicationError,
}

impl ClaimFailure {
    fn new(failure_code: impl Into<String>, error: ApplicationError) -> Self {
        Self { failure_code: failure_code.into(), error }
    }
}

pub(super) async fn process_claim_inner<C>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let retry_claim = claim.clone();
    match publish_claim(runtime, clock.as_ref(), claim, write_progress).await {
        Ok(()) => {
            write_progress.confirm_write();
            Ok(())
        }
        Err(ClaimFailure { failure_code, error }) => {
            record_failure(
                write_progress,
                Arc::clone(&runtime.store),
                clock,
                retry_claim,
                failure_code,
                error,
            )
            .await?;
            Err(WorkerError::ForumTopicProcessingFailed)
        }
    }
}

async fn publish_claim<C>(
    runtime: &ForumSyncRuntime,
    clock: &C,
    claim: ForumSyncClaim,
    write_progress: &ForumWriteProgress,
) -> Result<(), ClaimFailure>
where
    C: Clock + 'static,
{
    let reconciliation_only = requires_reconciliation(&claim);
    let retry_authorized = claim.previous_failure_code.as_deref() == Some("write_retry_authorized");
    let mut snapshot = runtime.store.load_snapshot(claim.clone()).await.map_err(|error| {
        ClaimFailure::new(scoped_failure_code(reconciliation_only, "snapshot_failed"), error)
    })?;
    snapshot.public_url =
        resolve_public_url(runtime.public_base_url.as_deref(), snapshot.public_url.as_deref())
            .map_err(|error| {
                ClaimFailure::new(scoped_failure_code(reconciliation_only, "configuration"), error)
            })?;
    let auction_id = snapshot.auction_id;
    let world = snapshot.world;
    let link_targets =
        snapshot.lots.iter().map(|lot| lot.equipment_link_target.clone()).collect::<Vec<_>>();
    let equipment_links =
        runtime.equipment_links.resolve_public_equipment_links(&link_targets).await.map_err(
            |error| {
                ClaimFailure::new(
                    scoped_failure_code(reconciliation_only, "equipment_links_failed"),
                    error,
                )
            },
        )?;
    let rendered = runtime
        .renderer
        .render(snapshot, equipment_links)
        .map_err(|error| render_failure(reconciliation_only, error))?;
    let catalog_page_count = rendered.catalog_continuation.len() + 1;
    let system_log = runtime
        .renderer
        .render_system_log(auction_id, catalog_page_count, catalog_page_count + 1, Vec::new())
        .map_err(|error| render_failure(reconciliation_only, error))?;
    let published = if reconciliation_only {
        match find_reconciliation_topic(runtime.publisher.as_ref(), world, &rendered).await {
            Ok(Some(published)) => ForumTopicPublication::Existing(published),
            // 已授权重试也只能在远端尚未开始写入时自动再次提交；一旦结果未知，
            // 下一轮必须重新回到精确标题只读对账。
            Ok(None) if retry_authorized => runtime
                .publisher
                .create_topic(world, rendered.clone(), write_progress.clone())
                .await
                .map_err(|error| ClaimFailure::new(create_failure_code(&error), error))?,
            Ok(None) => {
                return Err(ClaimFailure::new(
                    "write_authorization_required",
                    ApplicationError::Repository(RepositoryError::Failed(
                        "精确标题只读对账未发现主题，必须人工授权后才能重新发帖".to_owned(),
                    )),
                ));
            }
            Err(error) => {
                return Err(ClaimFailure::new(
                    scoped_failure_code(true, forum_failure_code(&error)),
                    error,
                ));
            }
        }
    } else {
        // 只要远端写入已经开始，后续状态一律先进入精确标题只读对账。
        runtime
            .publisher
            .create_topic(world, rendered.clone(), write_progress.clone())
            .await
            .map_err(|error| ClaimFailure::new(create_failure_code(&error), error))?
    };
    let (published, newly_created) = match published {
        ForumTopicPublication::Created(published) => (published, true),
        ForumTopicPublication::Existing(published) => (published, false),
    };
    // create_topic 也可能只找到旧主题；只有本轮真实创建或人工授权才允许补写固定楼层。
    let allow_layout_writes = newly_created || retry_authorized;
    let legacy_thread_id = claim.existing_thread_id.clone();
    let layout = ensure_fixed_layout(
        Arc::clone(&runtime.reader),
        Arc::clone(&runtime.post_writer),
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
    .map_err(|error| {
        // 已取得主题创建/查找证明；后续 GET 或固定楼层失败不能退回可自动发新主题的路径。
        let code = forum_failure_code(&error);
        // 人工补写授权使用已冻结的精确错误码，领取门禁会阻止自动重试。
        let failure_code = if code == "write_authorization_required" {
            code.to_owned()
        } else {
            scoped_failure_code(true, code)
        };
        ClaimFailure::new(failure_code, error)
    })?;
    runtime
        .store
        .publish(claim, rendered, system_log, layout, clock.now())
        .await
        .map_err(|error| ClaimFailure::new("publish_proof_failed", error))
}

/// 本地渲染失败不涉及远端写入；原因按安全分类收敛为仓储错误。
fn render_failure(reconciliation_only: bool, error: ForumRenderError) -> ClaimFailure {
    ClaimFailure::new(
        scoped_failure_code(reconciliation_only, render_failure_code(error)),
        ApplicationError::Repository(RepositoryError::Failed(error.to_string())),
    )
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
