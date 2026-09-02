//! 论坛主题读取、固定楼层校验、身份解析和目录/系统日志刷新。

use auction_application::{
    ForumCommandStatus, ForumRenderedPost, ForumSystemLogEntry, forum_catalog_marker,
    forum_system_log_marker,
};

use super::*;

pub(super) async fn process_topic(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
    clock: &dyn auction_application::Clock,
    poll_interval: chrono::Duration,
) -> Result<(), ApplicationError> {
    let preview = runtime.reader.fetch_topic(topic.world, topic.thread_id.clone()).await?;
    validate_fixed_layout(&topic, &preview)?;
    let post_numbers = preview
        .posts
        .iter()
        .enumerate()
        .map(|(index, post)| (post.post.id.clone(), index + 2))
        .collect::<HashMap<_, _>>();
    let mut system_entries = warning_entries(&preview, &post_numbers);
    let targets = runtime.store.lot_targets(topic.auction_id).await?;
    let target_by_code = target_map(targets)?;
    let mut command_error = None;
    let mut processed_through_post_number = 2_usize;

    'posts: for post in &preview.posts {
        let Some(post_number) = post_numbers.get(&post.post.id).copied() else {
            return Err(repository_error("论坛帖子缺少真实楼层映射"));
        };
        if post.post.id == topic.system_log_post_id {
            continue;
        }
        if !requires_bidder_identity(post) {
            processed_through_post_number = post_number;
            continue;
        }
        let Some(external_account_id) = post.post.author_id.clone() else {
            tracing::warn!(
                auction_id = %topic.auction_id,
                post_id = %post.post.id,
                "论坛帖子缺少可解析的论坛账号数字键，跳过命令"
            );
            system_entries.push(system_entry(
                post_number,
                &post.post.id,
                None,
                "identity_missing",
                "post author forum account ID could not be resolved",
                false,
            ));
            processed_through_post_number = post_number;
            continue;
        };
        let player_name = post.post.author.clone().unwrap_or_default();
        let bidder =
            match runtime.store.resolve_bidder(topic.world, external_account_id, player_name).await
            {
                Ok(bidder) => bidder,
                Err(error) => {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        post_id = %post.post.id,
                        error = %error,
                        "论坛帖子玩家名称无法解析为可竞价用户"
                    );
                    system_entries.push(system_entry(
                        post_number,
                        &post.post.id,
                        None,
                        "identity_unavailable",
                        "forum/MM player ID name could not be linked to an active auction user",
                        false,
                    ));
                    processed_through_post_number = post_number;
                    continue;
                }
            };
        for command in &post.commands {
            if matches!(command.kind.as_str(), "start" | "cancel") {
                system_entries.push(system_entry(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "admin_command_not_supported",
                    "start and cancel are Admin-only operations and were not applied",
                    false,
                ));
                continue;
            }
            let notify_mode = command.notify.as_deref();
            if matches!(notify_mode, Some("pm" | "both")) {
                system_entries.push(system_entry(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "forum_pm_unavailable",
                    "forum PM notifications are not available; MM processing is independent",
                    false,
                ));
            }
            if runtime.notifier.is_none() && matches!(notify_mode, Some("mm" | "both")) {
                system_entries.push(system_entry(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "mm_notification_unavailable",
                    "MoogleMail notification delivery is not configured",
                    false,
                ));
            }
            for lot_code in &command.lot_codes {
                let Some(&lot_id) = target_by_code.get(lot_code) else {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        lot_code = %lot_code,
                        "论坛命令 lot 编码未找到本地映射"
                    );
                    system_entries.push(system_entry(
                        post_number,
                        &post.post.id,
                        Some(command.line),
                        "lot_mapping_missing",
                        "lot code no longer maps to an active auction lot",
                        false,
                    ));
                    continue;
                };
                let context = ForumCommandContext {
                    topic: &topic,
                    bidder: &bidder,
                    post_id: &post.post.id,
                    post_number,
                    lot_code,
                    command,
                    lot_id,
                };
                if let Err(error) = process_command(runtime, &context).await {
                    // 保留帖子顺序：前一条暂时失败时不执行后续命令，但仍先把待重试状态投影到 #2。
                    system_entries.push(system_entry(
                        post_number,
                        &post.post.id,
                        Some(command.line),
                        "command_processing_pending",
                        "command processing stopped here and will retry",
                        false,
                    ));
                    command_error = Some(error);
                    break 'posts;
                }
            }
        }
        processed_through_post_number = post_number;
    }

    let outcomes =
        runtime.store.command_outcomes(topic.auction_id, topic.thread_id.clone()).await?;
    for outcome in outcomes {
        if outcome.status == ForumCommandStatus::Applied {
            continue;
        }
        let persisted_post_number =
            outcome.post_number.and_then(|value| usize::try_from(value).ok());
        let current_post_number = post_numbers.get(&outcome.post_id).copied();
        let post_number = persisted_post_number.or(current_post_number);
        if outcome.status == ForumCommandStatus::Pending {
            // pending 是未完成事实，即使帖子后来被编辑或删除，也不能把 #2 水位推进到它之后。
            let completed_floor = post_number.map_or(2, |value| value.saturating_sub(1).max(2));
            processed_through_post_number = processed_through_post_number.min(completed_floor);
            command_error.get_or_insert_with(|| repository_error("论坛命令账本仍有待重试记录"));
        }
        let Some(post_number) = post_number else { continue };
        let (code, detail) = match outcome.status {
            ForumCommandStatus::Pending => (
                "command_pending",
                outcome.safe_detail.as_deref().unwrap_or("processing will retry"),
            ),
            ForumCommandStatus::Rejected => (
                "command_rejected",
                outcome.safe_detail.as_deref().unwrap_or("command was rejected"),
            ),
            ForumCommandStatus::Applied => unreachable!("已在上方排除 applied 状态"),
        };
        let message = clean_system_message(&format!(
            "{} {}: {}",
            outcome.lot_code, outcome.command_kind, detail
        ));
        system_entries.push(system_entry(
            post_number,
            &outcome.post_id,
            usize::try_from(outcome.line_number).ok(),
            code,
            &message,
            false,
        ));
    }
    let scan_completed = command_error.is_none();
    refresh_fixed_posts(
        runtime,
        topic,
        processed_through_post_number,
        system_entries,
        clock,
        poll_interval,
        scan_completed,
    )
    .await?;
    command_error.map_or(Ok(()), Err)
}

fn requires_bidder_identity(post: &auction_application::AdminForumBidPost) -> bool {
    !post.ignored && !post.commands.is_empty()
}

fn validate_fixed_layout(
    topic: &auction_application::ForumReplyTopic,
    preview: &auction_application::AdminForumPreviewResult,
) -> Result<(), ApplicationError> {
    let catalog_marker = forum_catalog_marker(topic.auction_id);
    let system_marker = forum_system_log_marker(topic.auction_id);
    let Some(second) = preview.posts.first() else {
        return Err(repository_error("论坛固定 #2 已丢失"));
    };
    if preview.catalog_post_id != topic.catalog_post_id
        || !preview.catalog_post_text.contains(&catalog_marker)
        || second.post.id != topic.system_log_post_id
        || !second.post.text.contains(&system_marker)
    {
        return Err(repository_error("论坛固定 #1/#2 身份与持久化证明不一致"));
    }
    Ok(())
}

fn warning_entries(
    preview: &auction_application::AdminForumPreviewResult,
    post_numbers: &HashMap<String, usize>,
) -> Vec<ForumSystemLogEntry> {
    preview
        .warnings
        .iter()
        .filter_map(|warning| {
            let (post_number, post_id) = match warning.post_id.as_ref() {
                Some(post_id) => (post_numbers.get(post_id).copied()?, post_id.as_str()),
                None => (1, preview.catalog_post_id.as_str()),
            };
            (post_number == 1 || post_number >= 3).then(|| {
                system_entry(
                    post_number,
                    post_id,
                    (warning.line > 0).then_some(warning.line),
                    &warning.code,
                    &clean_system_message(&warning.message),
                    warning.code == "edited_post_ignored",
                )
            })
        })
        .collect()
}

async fn refresh_fixed_posts(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
    processed_through_post_number: usize,
    system_entries: Vec<ForumSystemLogEntry>,
    clock: &dyn auction_application::Clock,
    poll_interval: chrono::Duration,
    scan_completed: bool,
) -> Result<(), ApplicationError> {
    let mut snapshot = runtime.store.load_current_snapshot(topic.auction_id).await?;
    snapshot.public_url = super::super::forum_sync::resolve_public_url(
        runtime.public_base_url.as_deref(),
        snapshot.public_url.as_deref(),
    )?;
    let link_targets =
        snapshot.lots.iter().map(|lot| lot.equipment_link_target.clone()).collect::<Vec<_>>();
    let equipment_links =
        runtime.equipment_links.resolve_public_equipment_links(&link_targets).await?;
    let catalog = runtime
        .renderer
        .render(snapshot, equipment_links)
        .map_err(|error| repository_error(&error.to_string()))?;
    let system_log = runtime
        .renderer
        .render_system_log(topic.auction_id, processed_through_post_number, system_entries)
        .map_err(|error| repository_error(&error.to_string()))?;
    let catalog_changed = catalog.title != topic.title || catalog.body != topic.catalog_body;
    let system_log_changed = system_log.body != topic.system_log_body;
    if !catalog_changed && !system_log_changed {
        if !scan_completed {
            return Ok(());
        }
        let (updated_at, next_scan_at) = completed_scan_times(clock, poll_interval)?;
        return runtime.store.finish_topic_scan(topic, updated_at, next_scan_at).await;
    }
    if catalog_changed {
        let proof = runtime
            .writer
            .edit_post(
                topic.world,
                topic.thread_id.clone(),
                topic.catalog_post_id.clone(),
                Some(catalog.title.clone()),
                ForumRenderedPost { body: catalog.body.clone() },
            )
            .await?;
        if proof.thread_id != topic.thread_id || proof.post_id != topic.catalog_post_id {
            return Err(repository_error("论坛固定 #1 编辑证明不匹配"));
        }
    }
    if system_log_changed {
        let proof = runtime
            .writer
            .edit_post(
                topic.world,
                topic.thread_id.clone(),
                topic.system_log_post_id.clone(),
                None,
                system_log.clone(),
            )
            .await?;
        if proof.thread_id != topic.thread_id || proof.post_id != topic.system_log_post_id {
            return Err(repository_error("论坛固定 #2 编辑证明不匹配"));
        }
    }
    let updated_at = clock.now();
    if !scan_completed {
        return runtime
            .store
            .record_fixed_posts_progress(topic, catalog, system_log, updated_at)
            .await;
    }
    let next_scan_at = next_scan_at(updated_at, poll_interval)?;
    runtime
        .store
        .record_fixed_posts_update(topic, catalog, system_log, updated_at, next_scan_at)
        .await
}

fn completed_scan_times(
    clock: &dyn auction_application::Clock,
    poll_interval: chrono::Duration,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), ApplicationError> {
    let completed_at = clock.now();
    let next_scan_at = next_scan_at(completed_at, poll_interval)?;
    Ok((completed_at, next_scan_at))
}

fn next_scan_at(
    completed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    completed_at
        .checked_add_signed(poll_interval)
        .ok_or_else(|| repository_error("论坛下一次扫描时间超出可表示范围"))
}

fn system_entry(
    post_number: usize,
    post_id: &str,
    line_number: Option<usize>,
    code: &str,
    message: &str,
    ignored: bool,
) -> ForumSystemLogEntry {
    ForumSystemLogEntry {
        post_number,
        post_id: post_id.to_owned(),
        line_number,
        code: clean_system_message(code),
        message: clean_system_message(message),
        ignored,
    }
}

fn clean_system_message(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(512).collect()
}

fn target_map(
    targets: Vec<auction_application::ForumLotTarget>,
) -> Result<HashMap<String, auction_domain::LotId>, ApplicationError> {
    let mut result = HashMap::with_capacity(targets.len());
    for target in targets {
        if target.code.is_empty()
            || target.lot_id.is_nil()
            || result.insert(target.code, target.lot_id).is_some()
        {
            return Err(repository_error("论坛 lot 映射重复或无效"));
        }
    }
    if result.is_empty() {
        return Err(repository_error("论坛 lot 映射为空"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commandless_post_does_not_require_bidder_identity() {
        let post = auction_application::AdminForumBidPost {
            post: auction_application::AdminForumPost {
                id: "6956957".to_owned(),
                author_id: Some("5501029".to_owned()),
                author: Some("ngnlAYY".to_owned()),
                text: "One01 proxy 999m fast".to_owned(),
                edited: false,
            },
            commands: Vec::new(),
            warnings: Vec::new(),
            ignored: false,
        };

        assert!(!requires_bidder_identity(&post));
    }
}
