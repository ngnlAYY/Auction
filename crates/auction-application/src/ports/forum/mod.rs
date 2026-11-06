//! forum 上下文。

pub(crate) mod colors;
pub(crate) mod pm;
pub(crate) mod profile;
pub(crate) mod public;
pub(crate) mod reply;
pub(crate) mod sync;

pub use colors::{
    EquipmentColorDecision, EquipmentColorFacts, EquipmentColorMatch, EquipmentColorRule,
    EquipmentGroup, PEERLESS_COLORS, equipment_color_decision, validate_equipment_color_rules,
};
pub use pm::*;
pub use profile::*;
pub use public::{PUBLIC_FORUM_STATUSES, PublicForumLink, PublicForumReader, PublicForumThread};
pub use reply::{
    FORUM_POSTS_PER_PAGE, ForumActiveProxy, ForumBidCommandRecord, ForumBidderIdentity,
    ForumCatalogPage, ForumCommandOutcome, ForumCommandStart, ForumCommandStatus, ForumLotTarget,
    ForumReplyPhase, ForumReplyStore, ForumReplyTopic, ForumTopicCursor, ForumTopicReader,
    ForumTopicScan, NotifySubscription, forum_topic_fixed_page_count, forum_topic_scan_pages,
};
pub use sync::{
    FORUM_CATALOG_PAGE_SIZE, ForumAuctionSnapshot, ForumCatalogLayout, ForumCredentialAvailability,
    ForumCurrentBidSource, ForumLotSettlement, ForumLotSnapshot, ForumPostWriter,
    ForumPublishedLayout, ForumPublishedPost, ForumPublishedThread, ForumPublisher, ForumReadPost,
    ForumRemoteError, ForumRenderError, ForumRenderedPost, ForumRenderedThread, ForumRenderer,
    ForumSyncClaim, ForumSyncRecoveryItem, ForumSyncRecoveryPort, ForumSyncStore,
    ForumSystemLogEntry, ForumTopicMetadata, ForumTopicPublication, ForumWriteProgress,
    ForumWriteRetryAuthorizationCommand, ForumWriteRetryAuthorizationResult,
    MAX_FORUM_CATALOG_LOTS, MAX_FORUM_CATALOG_PAGES, MAX_FORUM_RECONCILIATION_TITLES,
    MAX_FORUM_SYNC_RECOVERY_ITEMS, forum_catalog_marker, forum_catalog_page_marker,
    forum_post_bodies_match, forum_system_log_marker,
};
