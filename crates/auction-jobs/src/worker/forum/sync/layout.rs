//! 固定楼层的顺序建立与 exact post 对账；不能覆盖抢楼或借重试移动系统日志。

use std::{borrow::Cow, sync::Arc};

use auction_application::{
    ApplicationError, ForumPostWriter, ForumPublishedLayout, ForumPublishedThread,
    ForumRemoteError, ForumRenderedPost, ForumRenderedThread, ForumTopicMetadata, ForumTopicReader,
    ForumWriteProgress, forum_catalog_marker, forum_catalog_page_marker, forum_system_log_marker,
};

pub(super) struct FixedLayoutRequest {
    pub world: auction_domain::SettlementWorld,
    pub auction_id: auction_domain::AuctionId,
    pub published: ForumPublishedThread,
    pub catalog: ForumRenderedThread,
    pub system_log: ForumRenderedPost,
    pub allow_layout_writes: bool,
    pub allow_content_update: bool,
    pub catalog_written: bool,
    pub verify_post_body: bool,
    pub legacy_thread_id: Option<String>,
    pub write_progress: ForumWriteProgress,
}

pub(super) async fn ensure_fixed_layout(
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
        write_progress,
    } = request;
    let mut preview = reader.fetch_topic(world, published.thread_id.clone()).await?;
    let catalog_marker = forum_catalog_marker(auction_id);
    // 请求已转移目录所有权；续页直接移动，避免额外复制整份 BBCode 正文。
    let mut replies = catalog
        .catalog_continuation
        .into_iter()
        .enumerate()
        .map(|(index, page)| (page, forum_catalog_page_marker(auction_id, index + 2)))
        .collect::<Vec<_>>();
    replies.push((system_log, forum_system_log_marker(auction_id)));

    // 先核对现存的整个固定区，再做任何写入；旧日志或用户占楼绝不能被改成装备页。
    for (index, (_, marker)) in replies.iter().enumerate() {
        if let Some(post) = preview.posts.get(index)
            && !post.post.text.contains(marker)
        {
            return Err(fixed_layout_error("fixed_layout_conflict", false));
        }
    }
    if !preview.catalog_post_text.contains(&catalog_marker) {
        if preview.catalog_post_text.contains("auction-catalog-v1:")
            || legacy_thread_id.as_deref() != Some(published.thread_id.as_str())
        {
            return Err(fixed_layout_error("fixed_layout_conflict", false));
        }
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
                write_progress.clone(),
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
    let policy = FixedPostPolicy {
        verify_body: verify_post_body,
        allow_update: allow_content_update,
        write_confirmed: catalog_written,
        write_progress: &write_progress,
    };
    if !verify_post_body && !allow_layout_writes {
        // 现存身份不证明未知 POST 的正文版本；关闭正文回读不能吞掉尚未发送的变化。
        return Err(fixed_layout_error("write_authorization_required", false));
    }
    reconcile_or_update_fixed_post(
        writer.as_ref(),
        world,
        &published.thread_id,
        &catalog_post_id,
        Some(ForumTopicMetadata {
            title: Some(catalog.title),
            description: Some(catalog.description),
        }),
        Cow::Owned(ForumRenderedPost { body: catalog.body }),
        policy,
    )
    .await?;

    let mut reply_ids = Vec::with_capacity(replies.len());
    for (index, (body, marker)) in replies.iter().enumerate() {
        let (post_id, written) = if let Some(post) = preview.posts.get(index) {
            if !post.post.text.contains(marker) {
                return Err(fixed_layout_error("fixed_layout_conflict", false));
            }
            (post.post.id.clone(), false)
        } else {
            if !allow_layout_writes {
                return Err(fixed_layout_error("write_authorization_required", false));
            }
            let proof = writer
                .reply_topic(
                    world,
                    published.thread_id.clone(),
                    body.clone(),
                    write_progress.clone(),
                )
                .await?;
            if proof.thread_id != published.thread_id {
                return Err(fixed_layout_error("write_unknown", true));
            }
            let confirmed = reader.fetch_topic(world, published.thread_id.clone()).await?;
            // 新回复必须落在预定楼层；每次重新读取也复核之前已确认的完整前缀。
            if confirmed.catalog_post_id != catalog_post_id
                || !confirmed.catalog_post_text.contains(&catalog_marker)
                || reply_ids.iter().enumerate().any(|(previous, post_id)| {
                    confirmed.posts.get(previous).is_none_or(|post| {
                        &post.post.id != post_id || !post.post.text.contains(&replies[previous].1)
                    })
                })
                || confirmed.posts.get(index).is_none_or(|post| {
                    post.post.id != proof.post_id || !post.post.text.contains(marker)
                })
            {
                return Err(fixed_layout_error("fixed_layout_conflict", true));
            }
            preview = confirmed;
            (proof.post_id, true)
        };
        if post_id == catalog_post_id || reply_ids.contains(&post_id) {
            return Err(fixed_layout_error("fixed_layout_conflict", written));
        }
        // 关闭正文校验且本轮已有回执时，原校验路径直接成功，无需再复制整页正文。
        if verify_post_body || !written {
            reconcile_or_update_fixed_post(
                writer.as_ref(),
                world,
                &published.thread_id,
                &post_id,
                None,
                Cow::Borrowed(body),
                FixedPostPolicy { write_confirmed: written, ..policy },
            )
            .await?;
        }
        reply_ids.push(post_id);
    }
    let system_log_post_id =
        reply_ids.pop().ok_or_else(|| fixed_layout_error("fixed_layout_conflict", false))?;
    Ok(ForumPublishedLayout {
        thread: published,
        catalog_post_id,
        catalog_continuation_post_ids: reply_ids,
        system_log_post_id,
    })
}

#[derive(Clone, Copy)]
struct FixedPostPolicy<'a> {
    verify_body: bool,
    allow_update: bool,
    /// 仅指本轮目标正文的创建/编辑/回复回执，不是历史帖子身份。
    write_confirmed: bool,
    write_progress: &'a ForumWriteProgress,
}

/// 已有远端布局证明后，按配置核对正文；任何内容更新仍须本轮人工授权。
/// 续页与日志先借用正文；只有确需编辑才取得所有权，已匹配或拒绝写入时不复制整页。
async fn reconcile_or_update_fixed_post(
    writer: &dyn ForumPostWriter,
    world: auction_domain::SettlementWorld,
    thread_id: &str,
    post_id: &str,
    metadata: Option<ForumTopicMetadata>,
    expected: Cow<'_, ForumRenderedPost>,
    policy: FixedPostPolicy<'_>,
) -> Result<(), ApplicationError> {
    if policy.verify_body {
        let observed = writer.read_post(world, thread_id.to_owned(), post_id.to_owned()).await?;
        if observed.matches(&expected.body, metadata.as_ref()) {
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
    let verification = policy.verify_body.then(|| (expected.body.clone(), metadata.clone()));
    let proof = writer
        .edit_post(
            world,
            thread_id.to_owned(),
            post_id.to_owned(),
            metadata,
            expected.into_owned(),
            policy.write_progress.clone(),
        )
        .await?;
    if proof.thread_id != thread_id || proof.post_id != post_id {
        return Err(fixed_layout_error("write_unknown", true));
    }
    if let Some((expected_body, metadata)) = verification {
        let confirmed = writer.read_post(world, thread_id.to_owned(), post_id.to_owned()).await?;
        if !confirmed.matches(&expected_body, metadata.as_ref()) {
            return Err(fixed_layout_error("write_unknown", true));
        }
    }
    Ok(())
}

fn fixed_layout_error(code: &'static str, write_started: bool) -> ApplicationError {
    ApplicationError::ForumRemote(ForumRemoteError::new(code, false, write_started))
}
