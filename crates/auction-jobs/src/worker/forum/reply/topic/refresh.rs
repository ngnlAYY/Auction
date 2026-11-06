//! 固定楼层刷新：目录、目录续页与系统日志的渲染、发布和提交。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::forum::{
    ForumRenderedPost, ForumSystemLogEntry, ForumTopicMetadata, ForumWriteProgress,
    forum_post_bodies_match,
};

use super::super::ForumReplyRuntime;
use super::super::common::repository_error;
use super::schedule::{completed_scan_times, next_scan_at};
use crate::worker::diagnostics;
use crate::worker::forum::sync::resolve_public_url;

pub(super) struct TopicScanProgress {
    pub(super) processed_through_post_number: usize,
    pub(super) system_entries: Vec<ForumSystemLogEntry>,
    pub(super) scan_completed: bool,
}

pub(super) async fn refresh_fixed_posts(
    runtime: &ForumReplyRuntime,
    mut topic: auction_application::forum::ForumReplyTopic,
    scan: TopicScanProgress,
    clock: &dyn auction_application::core::Clock,
    poll_interval: chrono::Duration,
    write_progress: &ForumWriteProgress,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApplicationError> {
    let TopicScanProgress { processed_through_post_number, system_entries, scan_completed } = scan;
    let processed_through_post_number_i32 =
        i32::try_from(processed_through_post_number).map_err(|error| {
            diagnostics::warn(
                "forum_reply",
                "convert_post_number",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            repository_error("论坛已处理楼层超出持久化范围")
        })?;
    // 这是完整主题快照的派生水位，不是历史命令楼层；删帖或未解决 pending 可使当前边界下降。
    // 存储仍通过内容 revision 和 claim CAS 防止过期扫描覆盖，命令账本的历史楼层不变。
    let mut snapshot = runtime.store.load_current_snapshot(topic.auction_id).await?;
    // pending 阻断扫描完成时只更新变化页，不能反复 POST 未变化的终态正文。
    let confirmed_terminal_version = (matches!(
        snapshot.state,
        auction_domain::AuctionStatus::Settled | auction_domain::AuctionStatus::Cancelled
    ) && scan_completed)
        .then_some(snapshot.state_version);
    snapshot.public_url =
        resolve_public_url(runtime.public_base_url.as_deref(), snapshot.public_url.as_deref())?;
    let link_targets =
        snapshot.lots.iter().map(|lot| lot.equipment_link_target.clone()).collect::<Vec<_>>();
    let equipment_links =
        runtime.equipment_links.resolve_public_equipment_links(&link_targets).await?;
    let catalog = runtime
        .renderer
        .render(snapshot, equipment_links)
        .map_err(|error| repository_error(&error.to_string()))?;
    if catalog.catalog_continuation.len() != topic.catalog_continuation.len() {
        return Err(repository_error("论坛已发布目录页数不能在刷新时改变"));
    }
    let system_log = runtime
        .renderer
        .render_system_log(
            topic.auction_id,
            topic.catalog_continuation.len() + 1,
            processed_through_post_number,
            system_entries,
        )
        .map_err(|error| repository_error(&error.to_string()))?;
    let catalog_changed = catalog.title != topic.title
        || !forum_post_bodies_match(&catalog.body, &topic.catalog_body);
    let continuation_changed = catalog
        .catalog_continuation
        .iter()
        .zip(&topic.catalog_continuation)
        .any(|(page, previous)| !forum_post_bodies_match(&page.body, &previous.body));
    let system_log_changed = !forum_post_bodies_match(&system_log.body, &topic.system_log_body);
    let floor_changed = processed_through_post_number_i32 != topic.processed_through_post_number;
    let reconciliation_only = topic.content_last_error_code.as_deref() == Some("write_unknown");
    if reconciliation_only || confirmed_terminal_version.is_some() {
        // 准备完成终态刷新时，即使本地正文相同也须确认全部固定帖，不能用 live 镜像伪造完成。
        // 未知结果不能仅凭本轮固定帖身份收敛，否则会把期间尚未发送的新出价镜像吞掉。
        // 启用校验时须逐字回读；关闭时保留未知门禁，两种模式下都绝不重发未知 POST。
        reconcile_or_edit_fixed_post(
            runtime,
            &topic,
            &topic.catalog_post_id,
            Some(ForumTopicMetadata {
                title: Some(catalog.title.clone()),
                description: Some(catalog.description.clone()),
            }),
            ForumRenderedPost { body: catalog.body.clone() },
            write_progress,
        )
        .await?;
        reconcile_or_edit_fixed_post(
            runtime,
            &topic,
            &topic.system_log_post_id,
            None,
            system_log.clone(),
            write_progress,
        )
        .await?;
    }
    // 续页同样受逐帖证明与未知写入栅栏保护；全部页面证明齐备后才推进本地版本/水位。
    for (page, previous) in catalog.catalog_continuation.iter().zip(&topic.catalog_continuation) {
        if reconciliation_only
            || confirmed_terminal_version.is_some()
            || !forum_post_bodies_match(&page.body, &previous.body)
        {
            reconcile_or_edit_fixed_post(
                runtime,
                &topic,
                &previous.post_id,
                None,
                page.clone(),
                write_progress,
            )
            .await?;
        }
    }
    // 仅在全部固定帖证明之后携带终态版本；存储完成入口再锁定拍卖复核。
    // scan 未完成时仅保存部分进度，绝不写终态完成版本。
    topic.confirmed_terminal_version = confirmed_terminal_version;
    if !catalog_changed && !continuation_changed && !system_log_changed {
        if !scan_completed {
            if !floor_changed {
                return Ok(None);
            }
            let updated_at = clock.now();
            runtime
                .store
                .record_fixed_posts_progress(
                    topic,
                    catalog,
                    system_log,
                    processed_through_post_number_i32,
                    updated_at,
                )
                .await?;
            write_progress.confirm_write();
            return Ok(None);
        }
        let (updated_at, next_scan_at) =
            completed_scan_times(runtime, &topic, clock, poll_interval).await?;
        runtime
            .store
            .finish_topic_scan(topic, processed_through_post_number_i32, updated_at, next_scan_at)
            .await?;
        write_progress.confirm_write();
        return Ok(Some(next_scan_at));
    }
    if catalog_changed && !reconciliation_only && confirmed_terminal_version.is_none() {
        reconcile_or_edit_fixed_post(
            runtime,
            &topic,
            &topic.catalog_post_id,
            Some(ForumTopicMetadata {
                title: Some(catalog.title.clone()),
                description: Some(catalog.description.clone()),
            }),
            ForumRenderedPost { body: catalog.body.clone() },
            write_progress,
        )
        .await?;
    }
    if system_log_changed && !reconciliation_only && confirmed_terminal_version.is_none() {
        reconcile_or_edit_fixed_post(
            runtime,
            &topic,
            &topic.system_log_post_id,
            None,
            system_log.clone(),
            write_progress,
        )
        .await?;
    }
    let updated_at = clock.now();
    if !scan_completed {
        runtime
            .store
            .record_fixed_posts_progress(
                topic,
                catalog,
                system_log,
                processed_through_post_number_i32,
                updated_at,
            )
            .await?;
        write_progress.confirm_write();
        return Ok(None);
    }
    let next_scan_at = next_scan_at(runtime, &topic, updated_at, poll_interval).await?;
    runtime
        .store
        .record_fixed_posts_update(
            topic,
            catalog,
            system_log,
            processed_through_post_number_i32,
            updated_at,
            next_scan_at,
        )
        .await?;
    write_progress.confirm_write();
    Ok(Some(next_scan_at))
}

async fn reconcile_or_edit_fixed_post(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::forum::ForumReplyTopic,
    post_id: &str,
    metadata: Option<ForumTopicMetadata>,
    expected: ForumRenderedPost,
    write_progress: &ForumWriteProgress,
) -> Result<(), ApplicationError> {
    // 未知写入始终只读核对，即使日常正文回读被关闭也不能凭帖子身份放行。
    if runtime.verify_post_body || topic.content_last_error_code.as_deref() == Some("write_unknown")
    {
        let current = runtime
            .writer
            .read_post(topic.world, topic.thread_id.clone(), post_id.to_owned())
            .await?;
        // 首帖正文相同不证明状态标题已更新；未知结果也必须取得真实元数据。
        if current.matches(&expected.body, metadata.as_ref()) {
            return Ok(());
        }
    }
    // 关闭正文校验不等于证明历史 POST 已完成；保留未知门禁，不重发，也不吞掉新的待发布镜像。
    if topic.content_last_error_code.as_deref() == Some("write_unknown") {
        return Err(auction_application::forum::ForumRemoteError::new(
            "write_unknown",
            false,
            true,
        )
        .into());
    }
    let verification = runtime.verify_post_body.then(|| (expected.body.clone(), metadata.clone()));
    let proof = runtime
        .writer
        .edit_post(
            topic.world,
            topic.thread_id.clone(),
            post_id.to_owned(),
            metadata,
            expected,
            write_progress.clone(),
        )
        .await?;
    if proof.thread_id != topic.thread_id || proof.post_id != post_id {
        return Err(auction_application::forum::ForumRemoteError::new(
            "write_unknown",
            false,
            true,
        )
        .into());
    }
    if let Some((expected_body, metadata)) = verification {
        let observed = runtime
            .writer
            .read_post(topic.world, topic.thread_id.clone(), post_id.to_owned())
            .await?;
        if !observed.matches(&expected_body, metadata.as_ref()) {
            return Err(auction_application::forum::ForumRemoteError::new(
                "write_unknown",
                false,
                true,
            )
            .into());
        }
    }
    Ok(())
}
