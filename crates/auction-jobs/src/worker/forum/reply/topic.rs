//! 论坛主题读取、身份解析和 lot 编码映射。

use super::*;

pub(super) async fn process_topic(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
) -> Result<(), ApplicationError> {
    let preview = runtime.reader.fetch_topic(topic.world, topic.thread_id.clone()).await?;
    let targets = runtime.store.lot_targets(topic.auction_id).await?;
    let target_by_code = target_map(targets)?;

    for post in preview.posts {
        if post.ignored {
            continue;
        }
        let Some(actor) = post.post.author_id.clone().or_else(|| post.post.author.clone()) else {
            tracing::warn!(
                auction_id = %topic.auction_id,
                post_id = %post.post.id,
                "论坛帖子缺少可解析的身份，跳过命令"
            );
            continue;
        };
        let display_name = post.post.author.clone().unwrap_or_else(|| actor.clone());
        let bidder =
            match runtime.store.resolve_bidder(topic.world, actor.clone(), display_name).await {
                Ok(bidder) => bidder,
                Err(error) => {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        post_id = %post.post.id,
                        error = %error,
                        "论坛帖子身份未通过登记映射"
                    );
                    continue;
                }
            };
        for command in post.commands {
            for lot_code in &command.lot_codes {
                let Some(&lot_id) = target_by_code.get(lot_code) else {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        lot_code = %lot_code,
                        "论坛命令 lot 编码未找到本地映射"
                    );
                    continue;
                };
                process_command(
                    runtime,
                    &topic,
                    &bidder,
                    &post.post.id,
                    lot_code,
                    &command,
                    lot_id,
                )
                .await?;
            }
        }
    }
    Ok(())
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
