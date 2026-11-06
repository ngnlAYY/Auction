//! forum 上下文公开 API。

pub use crate::ports::forum::{
    EquipmentColorDecision, EquipmentColorFacts, EquipmentColorMatch, EquipmentColorRule,
    EquipmentGroup, FORUM_CATALOG_PAGE_SIZE, FORUM_POSTS_PER_PAGE, ForumActiveProxy,
    ForumAuctionSnapshot, ForumBidCommandRecord, ForumBidderIdentity, ForumCatalogLayout,
    ForumCatalogPage, ForumCommandOutcome, ForumCommandStart, ForumCommandStatus,
    ForumCredentialAvailability, ForumCurrentBidSource, ForumLotSettlement, ForumLotSnapshot,
    ForumLotTarget, ForumPmError, ForumPmFuture, ForumPmMessage, ForumPmReceipt, ForumPmSender,
    ForumPostWriter, ForumPublishedLayout, ForumPublishedPost, ForumPublishedThread,
    ForumPublisher, ForumReadPost, ForumRemoteError, ForumRenderError, ForumRenderedPost,
    ForumRenderedThread, ForumRenderer, ForumReplyPhase, ForumReplyStore, ForumReplyTopic,
    ForumSyncClaim, ForumSyncRecoveryItem, ForumSyncRecoveryPort, ForumSyncStore,
    ForumSystemLogEntry, ForumTargetProfileEvidence, ForumTargetProfileReader, ForumTopicCursor,
    ForumTopicMetadata, ForumTopicPublication, ForumTopicReader, ForumTopicScan,
    ForumWriteProgress, ForumWriteRetryAuthorizationCommand, ForumWriteRetryAuthorizationResult,
    MAX_FORUM_CATALOG_LOTS, MAX_FORUM_CATALOG_PAGES, MAX_FORUM_RECONCILIATION_TITLES,
    MAX_FORUM_SYNC_RECOVERY_ITEMS, NotifySubscription, PEERLESS_COLORS, PUBLIC_FORUM_STATUSES,
    PreparedForumPm, PublicForumLink, PublicForumReader, PublicForumThread,
    equipment_color_decision, forum_catalog_marker, forum_catalog_page_marker,
    forum_post_bodies_match, forum_system_log_marker, forum_topic_fixed_page_count,
    forum_topic_scan_pages, validate_equipment_color_rules,
};
