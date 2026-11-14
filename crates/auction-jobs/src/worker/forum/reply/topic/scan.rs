//! 主题扫描：读取分页、校验固定楼层、消费回帖命令并投影系统日志。

use std::collections::HashMap;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};
use auction_application::forum::{ForumCommandStatus, ForumWriteProgress};

use super::super::command::process_command;
use super::super::completion::repository_error;
use super::super::rejection::{rejection_message, warning_message};
use super::super::{ForumCommandContext, ForumReplyRuntime};
use super::layout::{
    clean_system_message, runtime_warning, system_entry, validate_fixed_layout, warning_entries,
};
use super::refresh::{TopicScanProgress, refresh_fixed_posts};
use crate::worker::diagnostics;

#[tracing::instrument(

    skip_all,

    fields(

        auction_id = %topic.auction_id,

    )

)]
pub(in crate::worker::forum::reply) async fn process_topic(
    runtime: &ForumReplyRuntime,
    topic: auction_application::forum::ForumReplyTopic,
    clock: &dyn auction_application::core::Clock,
    poll_interval: chrono::Duration,
    write_progress: &ForumWriteProgress,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApplicationError> {
    let scan_started = std::time::Instant::now();
    // 游标只是已导入的最高楼层；固定楼层页仍必须重读，游标页再核对一次帖子身份。
    let cursor = runtime.store.reply_cursor(topic.auction_id, topic.thread_id.clone()).await?;
    let fixed_through_post_number =
        u32::try_from(topic.system_log_post_number()).unwrap_or(u32::MAX);
    let scan = match runtime
        .reader
        .fetch_topic_since(topic.world, topic.thread_id.clone(), cursor, fixed_through_post_number)
        .await
    {
        Ok(scan) => scan,
        Err(error) => {
            if let ApplicationError::ForumRemote(remote) = &error
                && remote.code() == "topic_page_limit"
                && let Some(page_count) = remote.page_count()
                && let Some(page_limit) = remote.page_limit()
                && let Err(record_error) = runtime
                    .store
                    .record_topic_page_count(topic.auction_id, page_count, page_limit)
                    .await
            {
                diagnostics::warn(
                    "forum_reply",
                    "record_topic_page_limit",
                    record_error.diagnostic_kind(),
                    DiagnosticContext {
                        auction_id: diagnostic_id(&topic.auction_id),
                        ..Default::default()
                    },
                );
            }
            return Err(error);
        }
    };
    let auction_application::forum::ForumTopicScan {
        preview,
        post_numbers,
        mode: _,
        page_count,
        page_limit,
    } = scan;
    if let Some(page_count) = page_count
        && let Some(page_limit) = page_limit
        && let Err(error) =
            runtime.store.record_topic_page_count(topic.auction_id, page_count, page_limit).await
    {
        diagnostics::warn(
            "forum_reply",
            "record_topic_page_count",
            error.diagnostic_kind(),
            DiagnosticContext {
                auction_id: diagnostic_id(&topic.auction_id),
                ..Default::default()
            },
        );
    }
    validate_fixed_layout(&topic, &preview)?;
    // 观测窗口覆盖本轮实际读取的分页；增量扫描看不到游标页之前的旧帖，但那部分已有历史窗口。
    let scan_observed = std::time::Instant::now();
    let delay_guard = runtime.reply_delay.begin_scan(
        topic.auction_id,
        topic.world,
        topic.reply_phase,
        scan_started,
        scan_observed,
        preview.posts.iter().map(|post| post.post.id.clone()),
    );
    // 楼层号来自分页偏移而不是本轮下标；增量扫描可能跳过中间分页。
    // 删帖可使当前读数下降，稳定命令身份始终由 post_id 与原负载核对。
    let post_numbers = preview
        .posts
        .iter()
        .zip(post_numbers)
        .map(|(post, post_number)| {
            (post.post.id.as_str(), usize::try_from(post_number).unwrap_or(usize::MAX))
        })
        .collect::<HashMap<_, _>>();
    let fixed_floor = topic.system_log_post_number();
    let mut system_entries = warning_entries(&topic, &preview, &post_numbers);
    let commands_open = runtime.store.reply_commands_open(topic.auction_id).await?;
    let target_by_code = if commands_open {
        target_map(runtime.store.lot_targets(topic.auction_id).await?)?
    } else {
        // 流拍场也能刷新；终态不需要可竞价 lot 映射，更不能解析身份或重放回帖。
        system_entries
            .retain(|entry| entry.post_number <= topic.processed_through_post_number as usize);
        HashMap::new()
    };
    // 先读持久 pending 边界，不能等较晚竞价已提交后才压低展示水位。
    let pending = runtime.store.command_outcomes(topic.auction_id, topic.thread_id.clone()).await?;
    let mut pending_boundary = pending
        .iter()
        .filter(|outcome| outcome.status == ForumCommandStatus::Pending)
        .map(|outcome| post_numbers.get(outcome.post_id.as_str()).copied().unwrap_or(fixed_floor))
        .min();
    let mut command_error = None;
    let mut processed_through_post_number =
        if commands_open { fixed_floor } else { topic.processed_through_post_number as usize };

    'posts: for post in &preview.posts {
        if !commands_open {
            break;
        }
        let Some(post_number) = post_numbers.get(post.post.id.as_str()).copied() else {
            return Err(repository_error("论坛帖子缺少真实楼层映射"));
        };
        if post.post.id == topic.system_log_post_id
            || topic.catalog_continuation.iter().any(|page| page.post_id == post.post.id)
        {
            continue;
        }
        // 领取时 live 不保证扫描时仍 live；停止新回帖消费，最终快照仍须走全页证明。
        // begin_command 在拍卖锁内再次复核，覆盖此只读检查之后的状态竞争。
        if !runtime.store.reply_commands_open(topic.auction_id).await? {
            processed_through_post_number =
                processed_through_post_number.max(topic.processed_through_post_number as usize);
            break 'posts;
        }
        // 已删除的 pending 帖把边界设为系统日志楼层；编辑后无命令的原帖也必须阻挡后续楼层。
        if pending_boundary.is_some_and(|boundary| {
            post_number > boundary || post_number == boundary && !requires_bidder_identity(post)
        }) {
            command_error = Some(repository_error("论坛持久 pending 帖尚未解决，停止后续楼层"));
            break 'posts;
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
            system_entries.push(runtime_warning(
                post_number,
                &post.post.id,
                None,
                "identity_missing",
            ));
            processed_through_post_number = post_number;
            continue;
        };
        let player_name = post.post.author.clone().unwrap_or_default();
        let bidder =
            match runtime.store.resolve_bidder(topic.world, external_account_id, player_name).await
            {
                Ok(bidder) => bidder,
                Err(error @ ApplicationError::ForumIdentityRejected(_)) => {
                    diagnostics::warn(
                        "forum_reply",
                        "reject_bidder_identity",
                        error.diagnostic_kind(),
                        DiagnosticContext {
                            auction_id: diagnostic_id(&topic.auction_id),
                            ..Default::default()
                        },
                    );
                    system_entries.push(runtime_warning(
                        post_number,
                        &post.post.id,
                        None,
                        "identity_unavailable",
                    ));
                    processed_through_post_number = post_number;
                    continue;
                }
                Err(error) => {
                    // 只有权威身份拒绝允许跳过；暂时存储失败不能让较晚楼层先接受竞价。
                    diagnostics::warn(
                        "forum_reply",
                        "resolve_bidder_identity",
                        error.diagnostic_kind(),
                        DiagnosticContext {
                            auction_id: diagnostic_id(&topic.auction_id),
                            ..Default::default()
                        },
                    );
                    system_entries.push(runtime_warning(
                        post_number,
                        &post.post.id,
                        None,
                        "identity_processing_pending",
                    ));
                    command_error = Some(error);
                    break 'posts;
                }
            };
        for command in &post.commands {
            if matches!(command.kind.as_str(), "start" | "cancel") {
                system_entries.push(runtime_warning(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "admin_command_not_supported",
                ));
                continue;
            }
            // notify 只建立关注订阅，不再依赖 MoogleMail/PM owner 是否装配。
            for lot_code in &command.lot_codes {
                let Some(&lot_id) = target_by_code.get(lot_code) else {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        lot_code = %lot_code,
                        "论坛命令 lot 编码未找到本地映射"
                    );
                    system_entries.push(runtime_warning(
                        post_number,
                        &post.post.id,
                        Some(command.line),
                        "lot_mapping_missing",
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
                    // 保留帖子顺序：前一条暂时失败时不执行后续命令，但仍把待重试状态投影到系统日志。
                    system_entries.push(runtime_warning(
                        post_number,
                        &post.post.id,
                        Some(command.line),
                        "command_processing_pending",
                    ));
                    command_error = Some(error);
                    break 'posts;
                }
            }
        }
        if pending_boundary == Some(post_number) {
            let pending =
                runtime.store.command_outcomes(topic.auction_id, topic.thread_id.clone()).await?;
            pending_boundary = pending
                .iter()
                .filter(|outcome| outcome.status == ForumCommandStatus::Pending)
                .map(|outcome| {
                    post_numbers.get(outcome.post_id.as_str()).copied().unwrap_or(fixed_floor)
                })
                .min();
            if pending_boundary.is_some_and(|boundary| boundary <= post_number) {
                command_error = Some(repository_error("论坛原命令未收口，停止后续楼层"));
                break 'posts;
            }
        }
        processed_through_post_number = post_number;
    }

    let outcomes =
        runtime.store.command_outcomes(topic.auction_id, topic.thread_id.clone()).await?;
    for outcome in outcomes {
        let persisted_post_number =
            outcome.post_number.and_then(|value| usize::try_from(value).ok());
        let current_post_number = post_numbers.get(outcome.post_id.as_str()).copied();
        let post_number = persisted_post_number.or(current_post_number);
        if outcome.status == ForumCommandStatus::Pending {
            // pending 是未完成事实，即使帖子后来被编辑或删除，也不能把更新水位推进到它之后。
            let completed_floor = current_post_number
                .map_or(fixed_floor, |value| value.saturating_sub(1).max(fixed_floor));
            processed_through_post_number = processed_through_post_number.min(completed_floor);
            command_error.get_or_insert_with(|| repository_error("论坛命令账本仍有待重试记录"));
        }
        let Some(post_number) = post_number else { continue };
        // 原始诊断只留本地；错误码及可识别的旧拒绝详情只能映射为白名单英文，不重放命令。
        let (code, detail) = match outcome.status {
            ForumCommandStatus::Pending => warning_message("command_pending"),
            ForumCommandStatus::Rejected => {
                rejection_message(outcome.error_code.as_deref(), outcome.safe_detail.as_deref())
            }
            ForumCommandStatus::Applied => continue,
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
    delay_guard.finish(scan_completed);
    let next_scan_at = refresh_fixed_posts(
        runtime,
        topic,
        TopicScanProgress { processed_through_post_number, system_entries, scan_completed },
        clock,
        poll_interval,
        write_progress,
    )
    .await?;
    command_error.map_or(Ok(next_scan_at), Err)
}

fn requires_bidder_identity(post: &auction_application::admin::AdminForumBidPost) -> bool {
    !post.ignored && !post.commands.is_empty()
}

fn target_map(
    targets: Vec<auction_application::forum::ForumLotTarget>,
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
        let post = auction_application::admin::AdminForumBidPost {
            post: auction_application::admin::AdminForumPost {
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
