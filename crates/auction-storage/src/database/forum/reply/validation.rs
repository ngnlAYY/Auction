//! 论坛回帖存储的输入闭集、文本边界和能力错误。

use auction_application::core::ApplicationError;
use auction_application::forum::{ForumBidCommandRecord, ForumReplyTopic};
use auction_domain::SettlementWorld;
pub(super) use auction_domain::valid_text;
use auction_time::Timestamp;

pub(super) const MAX_REPLY_BATCH_SIZE: u16 = 100;
pub(super) const MAX_PLAYER_NAME_BYTES: usize = 256;
pub(super) const MAX_FORUM_ACCOUNT_ID_BYTES: usize = 32;
pub(super) const MAX_THREAD_ID_BYTES: usize = 64;
pub(super) const MAX_FIXED_POST_ID_BYTES: usize = 32;
pub(super) const MAX_POST_ID_BYTES: usize = 256;
pub(super) const MAX_LOT_CODE_BYTES: usize = 16;
pub(super) const MAX_ACTOR_BYTES: usize = 256;
pub(super) const MAX_COMMAND_KIND_BYTES: usize = 32;
pub(super) const MAX_ERROR_CODE_BYTES: usize = 128;
pub(super) const MAX_ERROR_DETAIL_BYTES: usize = 2048;

pub(super) fn valid_command_record(record: &ForumBidCommandRecord) -> bool {
    !record.command_id.is_nil()
        && !record.auction_id.is_nil()
        && valid_text(&record.thread_id, MAX_THREAD_ID_BYTES)
        && valid_text(&record.post_id, MAX_POST_ID_BYTES)
        && record.post_number >= 3
        && record.line_number > 0
        && valid_text(&record.lot_code, MAX_LOT_CODE_BYTES)
        && valid_text(&record.actor, MAX_ACTOR_BYTES)
        && matches!(record.command_kind.as_str(), "bid" | "proxy" | "proxy_lower" | "notify")
        && record.command_kind.len() <= MAX_COMMAND_KIND_BYTES
        && record.amount >= 0
}

/// 固定楼层写端口共用的主题租约校验：拍卖与主题身份、claim 三元组有效，且租约晚于本次
/// 操作时刻。四个写端口的拒绝口径只在这里定义。
pub(super) fn valid_topic_lease(topic: &ForumReplyTopic, at: Timestamp) -> bool {
    !topic.auction_id.is_nil()
        && valid_decimal_id(&topic.thread_id, MAX_THREAD_ID_BYTES)
        && !topic.content_claim_token.is_nil()
        && valid_text(&topic.content_claim_owner, 256)
        && topic.content_claim_fencing > 0
        && topic.content_lease_until > at
}

/// 固定楼层布局：目录与系统日志帖子 ID、续页闭集与内容版本有效。
pub(super) fn valid_topic_layout(topic: &ForumReplyTopic) -> bool {
    valid_decimal_id(&topic.catalog_post_id, MAX_FIXED_POST_ID_BYTES)
        && valid_decimal_id(&topic.system_log_post_id, MAX_FIXED_POST_ID_BYTES)
        && super::super::catalog::valid_pages(
            &topic.catalog_post_id,
            &topic.system_log_post_id,
            &topic.catalog_continuation,
        )
        && topic.content_revision > 0
}

/// 已处理水位不得低于系统日志楼层：固定楼层之后才是玩家回帖。
pub(super) fn valid_processed_through(
    topic: &ForumReplyTopic,
    processed_through_post_number: i32,
) -> bool {
    processed_through_post_number >= topic.system_log_post_number() as i32
}

pub(super) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

pub(super) use super::super::sync::validation::valid_decimal_id;

pub(super) fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    super::super::sync::parse_world(value)
}

#[cfg(test)]
mod tests {
    use auction_application::forum::{
        ForumCatalogLayout, ForumCatalogPage, ForumReplyPhase, ForumReplyTopic,
    };
    use auction_domain::{AuctionId, SettlementWorld};
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::{valid_processed_through, valid_topic_layout, valid_topic_lease};

    fn topic(now: Timestamp) -> ForumReplyTopic {
        ForumReplyTopic {
            auction_id: AuctionId::from_uuid(Uuid::from_u128(1)),
            world: SettlementWorld::Main,
            reply_phase: ForumReplyPhase::Normal,
            thread_id: "1001".to_owned(),
            catalog_post_id: "2001".to_owned(),
            catalog_layout: ForumCatalogLayout::PagedV1,
            catalog_continuation: vec![ForumCatalogPage {
                post_id: "2002".to_owned(),
                body: "continued catalog".to_owned(),
            }],
            system_log_post_id: "2003".to_owned(),
            title: "auction".to_owned(),
            catalog_body: "catalog".to_owned(),
            system_log_body: "log".to_owned(),
            content_revision: 1,
            confirmed_terminal_version: None,
            processed_through_post_number: 3,
            content_claim_token: Uuid::from_u128(2),
            content_claim_owner: "worker-1".to_owned(),
            content_claim_fencing: 1,
            content_lease_until: now + Duration::seconds(30),
            content_last_error_code: None,
        }
    }

    #[test]
    fn topic_lease_requires_identity_claim_and_unexpired_lease() {
        let now = Timestamp::now();
        let valid = topic(now);
        assert!(valid_topic_lease(&valid, now));
        let rejected: [fn(&mut ForumReplyTopic); 5] = [
            |topic| topic.auction_id = AuctionId::from_uuid(Uuid::nil()),
            |topic| topic.thread_id = "thread".to_owned(),
            |topic| topic.content_claim_token = Uuid::nil(),
            |topic| topic.content_claim_owner = String::new(),
            |topic| topic.content_claim_fencing = 0,
        ];
        for mutate in rejected {
            let mut topic = valid.clone();
            mutate(&mut topic);
            assert!(!valid_topic_lease(&topic, now), "{topic:?}");
        }
        // 租约必须严格晚于操作时刻。
        assert!(!valid_topic_lease(&valid, valid.content_lease_until));
    }

    #[test]
    fn topic_layout_and_processed_watermark_follow_the_fixed_posts() {
        let now = Timestamp::now();
        let valid = topic(now);
        assert!(valid_topic_layout(&valid));
        let mut duplicate_page = valid.clone();
        duplicate_page.system_log_post_id = "2002".to_owned();
        assert!(!valid_topic_layout(&duplicate_page));
        let mut unrevised = valid.clone();
        unrevised.content_revision = 0;
        assert!(!valid_topic_layout(&unrevised));

        // 一页续页时系统日志在 #3：水位不得低于它。
        assert!(valid_processed_through(&valid, 3));
        assert!(!valid_processed_through(&valid, 2));
    }
}
