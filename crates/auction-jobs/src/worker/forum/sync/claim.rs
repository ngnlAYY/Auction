//! 单条主题的远端编排：快照渲染、创建/精确标题对账、固定楼层与发布证明。

use std::sync::Arc;

use auction_application::core::{ApplicationError, Clock, RepositoryError};
use auction_application::forum::{
    ForumPublishedThread, ForumPublisher, ForumRenderedThread, ForumSyncClaim,
    ForumTopicPublication, ForumWriteProgress,
};

use super::failure::{
    forum_failure_code, record_failure, requires_reconciliation, scoped_failure_code,
};
use super::layout::{FixedLayoutRequest, ensure_fixed_layout};
use super::links::resolve_public_url;
use super::runtime::ForumSyncRuntime;
use crate::worker::runtime::WorkerError;

pub(super) async fn process_claim_inner<C>(
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
