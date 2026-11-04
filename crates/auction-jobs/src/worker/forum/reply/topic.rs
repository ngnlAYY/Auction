//! 论坛主题读取、固定楼层校验、身份解析和目录/系统日志刷新。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};
use auction_application::{
    ForumCommandStatus, ForumRenderedPost, ForumSystemLogEntry, ForumTopicMetadata,
    forum_catalog_marker, forum_post_bodies_match, forum_system_log_marker,
};

use super::*;
use crate::worker::diagnostics;

/// Final 阶段的最短导入间隔；截止前后都保持该节奏直到拍卖离开 live/final。
const FINAL_SCAN_MINIMUM_INTERVAL_SECONDS: i64 = 5;
/// Final 阶段按最早 lot 有效截止剩余时间收紧间隔的除数。
const FINAL_SCAN_REMAINING_DIVISOR: i32 = 6;

#[tracing::instrument(

    skip_all,

    fields(

        auction_id = %topic.auction_id,

    )

)]
pub(super) async fn process_topic(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
    clock: &dyn auction_application::Clock,
    poll_interval: chrono::Duration,
    write_progress: &ForumWriteProgress,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApplicationError> {
    let scan_started = std::time::Instant::now();
    // 游标只是已导入的最高楼层；固定楼层页仍必须重读，游标页再核对一次帖子身份。
    let cursor = runtime.store.reply_cursor(topic.auction_id, topic.thread_id.clone()).await?;
    let fixed_through_post_number =
        u32::try_from(topic.system_log_post_number()).unwrap_or(u32::MAX);
    let scan = runtime
        .reader
        .fetch_topic_since(topic.world, topic.thread_id.clone(), cursor, fixed_through_post_number)
        .await?;
    let auction_application::ForumTopicScan { preview, post_numbers, mode: _ } = scan;
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
            let notify_mode = command.notify.as_deref();
            if matches!(notify_mode, Some("pm" | "both")) {
                system_entries.push(runtime_warning(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "forum_pm_unavailable",
                ));
            }
            if runtime.notifier.is_none() && matches!(notify_mode, Some("mm" | "both")) {
                system_entries.push(runtime_warning(
                    post_number,
                    &post.post.id,
                    Some(command.line),
                    "mm_notification_unavailable",
                ));
            }
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
            ForumCommandStatus::Pending => super::rejection::warning_message("command_pending"),
            ForumCommandStatus::Rejected => super::rejection::rejection_message(
                outcome.error_code.as_deref(),
                outcome.safe_detail.as_deref(),
            ),
            // 已应用命令不产生系统日志条目。
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

fn requires_bidder_identity(post: &auction_application::AdminForumBidPost) -> bool {
    !post.ignored && !post.commands.is_empty()
}

fn validate_fixed_layout(
    topic: &auction_application::ForumReplyTopic,
    preview: &auction_application::AdminForumPreviewResult,
) -> Result<(), ApplicationError> {
    let catalog_marker = forum_catalog_marker(topic.auction_id);
    let system_marker = forum_system_log_marker(topic.auction_id);
    let system_index = topic.catalog_continuation.len();
    let Some(system) = preview.posts.get(system_index) else {
        return Err(repository_error("论坛系统更新楼已丢失"));
    };
    if preview.catalog_post_id != topic.catalog_post_id
        || !preview.catalog_post_text.contains(&catalog_marker)
        || system.post.id != topic.system_log_post_id
        || !system.post.text.contains(&system_marker)
        || preview.catalog.lots.len().div_ceil(topic.catalog_layout.page_size()) != system_index + 1
    {
        return Err(repository_error("论坛固定楼层或目录页数与持久化证明不一致"));
    }
    for (index, page) in topic.catalog_continuation.iter().enumerate() {
        let Some(post) = preview.posts.get(index) else {
            return Err(repository_error("论坛目录续页已丢失"));
        };
        if post.post.id != page.post_id
            || !post.post.text.contains(&auction_application::forum_catalog_page_marker(
                topic.auction_id,
                index + 2,
            ))
        {
            return Err(repository_error("论坛目录续页身份与持久化证明不一致"));
        }
    }
    Ok(())
}

fn warning_entries(
    topic: &auction_application::ForumReplyTopic,
    preview: &auction_application::AdminForumPreviewResult,
    post_numbers: &HashMap<&str, usize>,
) -> Vec<ForumSystemLogEntry> {
    preview
        .warnings
        .iter()
        .filter_map(|warning| {
            let (post_number, post_id) = match warning.post_id.as_ref() {
                Some(post_id) => (post_numbers.get(post_id.as_str()).copied()?, post_id.as_str()),
                None => (1, preview.catalog_post_id.as_str()),
            };
            let catalog_warning = post_number < topic.system_log_post_number()
                && matches!(warning.code.as_str(), "invalid_catalog_row" | "invalid_start_price");
            (catalog_warning || post_number > topic.system_log_post_number()).then(|| {
                let (code, message) = super::rejection::warning_message(&warning.code);
                system_entry(
                    post_number,
                    post_id,
                    (warning.line > 0).then_some(warning.line),
                    code,
                    message,
                    warning.code == "edited_post_ignored",
                )
            })
        })
        .collect()
}

struct TopicScanProgress {
    processed_through_post_number: usize,
    system_entries: Vec<ForumSystemLogEntry>,
    scan_completed: bool,
}

async fn refresh_fixed_posts(
    runtime: &ForumReplyRuntime,
    mut topic: auction_application::ForumReplyTopic,
    scan: TopicScanProgress,
    clock: &dyn auction_application::Clock,
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
    topic: &auction_application::ForumReplyTopic,
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
        return Err(auction_application::ForumRemoteError::new("write_unknown", false, true).into());
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
        return Err(auction_application::ForumRemoteError::new("write_unknown", false, true).into());
    }
    if let Some((expected_body, metadata)) = verification {
        let observed = runtime
            .writer
            .read_post(topic.world, topic.thread_id.clone(), post_id.to_owned())
            .await?;
        if !observed.matches(&expected_body, metadata.as_ref()) {
            return Err(
                auction_application::ForumRemoteError::new("write_unknown", false, true).into()
            );
        }
    }
    Ok(())
}

async fn completed_scan_times(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::ForumReplyTopic,
    clock: &dyn auction_application::Clock,
    poll_interval: chrono::Duration,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), ApplicationError> {
    let completed_at = clock.now();
    let next_scan_at = next_scan_at(runtime, topic, completed_at, poll_interval).await?;
    Ok((completed_at, next_scan_at))
}

/// 计算下一次导入时刻；Final 阶段按最早 lot 有效截止收紧间隔。
async fn next_scan_at(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::ForumReplyTopic,
    completed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    let interval = if topic.reply_phase == auction_application::ForumReplyPhase::Final {
        match runtime.store.earliest_live_lot_deadline(topic.auction_id).await? {
            Some(earliest_deadline) => {
                final_scan_interval(completed_at, earliest_deadline, poll_interval)
            }
            None => poll_interval,
        }
    } else {
        poll_interval
    };
    completed_at
        .checked_add_signed(interval)
        .ok_or_else(|| repository_error("论坛下一次扫描时间超出可表示范围"))
}

/// Final 阶段把导入间隔收紧到“最早 lot 有效截止剩余时间 / 6”，
/// 并夹在最短间隔与常规轮询间隔之间。截止已过时固定为最短间隔，
/// 直到拍卖离开 live/final；这里只改变读取节奏，不改变任何裁决时刻。
fn final_scan_interval(
    completed_at: chrono::DateTime<chrono::Utc>,
    earliest_deadline: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
) -> chrono::Duration {
    let minimum = chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS);
    if poll_interval <= minimum {
        return poll_interval;
    }
    // 负数（已过截止）也按最短间隔处理，不能给远端负延迟。
    let remaining = earliest_deadline
        .signed_duration_since(completed_at)
        .checked_div(FINAL_SCAN_REMAINING_DIVISOR)
        .unwrap_or(minimum);
    if remaining < minimum {
        minimum
    } else if remaining > poll_interval {
        poll_interval
    } else {
        remaining
    }
}

fn runtime_warning(
    post_number: usize,
    post_id: &str,
    line_number: Option<usize>,
    code: &str,
) -> ForumSystemLogEntry {
    let (code, message) = super::rejection::warning_message(code);
    system_entry(post_number, post_id, line_number, code, message, false)
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
    value
        .split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
        .take(512)
        .collect()
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
    fn final_scan_interval_tightens_near_earliest_deadline() {
        let completed_at = chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00Z")
            .unwrap_or_else(|error| panic!("时间应合法: {error:?}"))
            .with_timezone(&chrono::Utc);
        let poll = chrono::Duration::minutes(5);

        // 剩余 60 分钟时仍受常规轮询间隔限制。
        let far = completed_at + chrono::Duration::minutes(60);
        assert_eq!(final_scan_interval(completed_at, far, poll), poll);

        // 剩余 60 秒时按 1/6 收紧到 10 秒。
        let near = completed_at + chrono::Duration::seconds(60);
        assert_eq!(final_scan_interval(completed_at, near, poll), chrono::Duration::seconds(10));

        // 剩余不足 30 秒时收敛到最短间隔，而不是继续线性缩小。
        let imminent = completed_at + chrono::Duration::seconds(12);
        assert_eq!(
            final_scan_interval(completed_at, imminent, poll),
            chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS)
        );

        // 截止已过仍保持最短间隔，等到拍卖离开 live/final。
        let past = completed_at - chrono::Duration::minutes(3);
        assert_eq!(
            final_scan_interval(completed_at, past, poll),
            chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS)
        );
    }

    #[test]
    fn final_scan_interval_never_exceeds_poll_interval() {
        let completed_at = chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00Z")
            .unwrap_or_else(|error| panic!("时间应合法: {error:?}"))
            .with_timezone(&chrono::Utc);
        // 轮询间隔本身低于最短间隔时不放大读取频率。
        let short_poll = chrono::Duration::seconds(3);
        let near = completed_at + chrono::Duration::seconds(6);
        assert_eq!(final_scan_interval(completed_at, near, short_poll), short_poll);
    }

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
