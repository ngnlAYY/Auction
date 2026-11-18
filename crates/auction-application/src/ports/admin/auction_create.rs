//! Admin 创建拍卖的命令与事务端口。

use std::future::Future;

use auction_domain::{AuctionId, CommandId, valid_text};
use auction_time::Timestamp;
use uuid::Variant;

use crate::ports::admin::AdminAuctionSummary;
use crate::ports::auction::AuctionPolicyTemplate;
use crate::ports::core::ApplicationFuture;
use crate::ports::core::{CompetitionTransactionError, RequestHash};

const MAX_AUCTION_SEQUENCE: i64 = 9_007_199_254_740_991;

/// Admin 创建拍卖（`POST /api/v1/admin/auctions`）的命令。策略快照由应用服务在事务前按当前
/// 已冻结的默认策略构造并写入，HTTP 请求本身不能注入未审计的数据库字段或外部协议数据。
#[derive(Debug, Clone)]
pub struct AdminCreateAuctionCommand {
    pub command_id: CommandId,
    pub auction_id: AuctionId,
    pub world_id: String,
    pub sequence: i64,
    pub preview_scheduled_at: Timestamp,
    pub scheduled_open_at: Timestamp,
    pub scheduled_end_at: Timestamp,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminCreateAuctionCommand {
    /// 校验不依赖数据库状态的命令字段。
    pub fn validate(&self) -> bool {
        let auction_id = self.auction_id.as_uuid();
        // 新场次必须可由两版 Public 与公开事件读取；违反契约的命令在写入前拒绝。
        !self.command_id.is_nil()
            && auction_id.get_variant() == Variant::RFC4122
            && (1..=5).contains(&auction_id.get_version_num())
            && valid_text(&self.world_id, 64)
            && (1..=MAX_AUCTION_SEQUENCE).contains(&self.sequence)
            && self.expected_state_version == 0
            && valid_time(self.preview_scheduled_at)
            && valid_time(self.scheduled_open_at)
            && valid_time(self.scheduled_end_at)
            && self.preview_scheduled_at < self.scheduled_open_at
            && self.scheduled_end_at > self.scheduled_open_at
            && valid_text(&self.actor_id, 128)
            && self.request_id.as_deref().is_none_or(|value| valid_text(value, 128))
    }

    /// 为当前 schema 仍要求的内部标题生成稳定值；旧 API 没有单独的 title 字段。
    pub fn generated_title(&self) -> String {
        format!("{} #{}", self.world_id, self.sequence)
    }
}

pub trait AdminAuctionCreatePort: Send + Sync {
    fn create(
        &self,
        command: AdminCreateAuctionCommand,
    ) -> ApplicationFuture<'_, AdminAuctionSummary>;
}

/// Admin 创建拍卖事务的 PostgreSQL 窄适配器能力。
pub trait AdminAuctionCreateTransaction: Send {
    /// 按 world → intake target 顺序锁定创建资格；同一事务随后才读取当前时间并执行创建。
    /// 已接受命令先重放，不得为旧结果重新检查世界状态或计划时间。
    fn lock_admin_auction_creation(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    /// 使用本事务已经锁定的创建上下文写入聚合，不再次等待业务锁。
    fn create_admin_auction(
        &mut self,
        command: AdminCreateAuctionCommand,
        title: String,
        policy: AuctionPolicyTemplate,
        created_at: Timestamp,
    ) -> impl Future<Output = Result<AdminAuctionSummary, CompetitionTransactionError>> + Send;

    fn append_admin_auction_created_record(
        &mut self,
        command: AdminCreateAuctionCommand,
        result: AdminAuctionSummary,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

fn valid_time(value: Timestamp) -> bool {
    value.timestamp_micros() != 0
}

#[cfg(test)]
mod tests {
    use auction_domain::AuctionId;
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::AdminCreateAuctionCommand;
    use crate::ports::core::RequestHash;

    fn command() -> AdminCreateAuctionCommand {
        let preview = Timestamp::from_timestamp(1_800_000_000, 0).unwrap();
        AdminCreateAuctionCommand {
            command_id: Uuid::parse_str("01976b9c-af94-7b04-89fa-5b49708c9faa").unwrap().into(),
            auction_id: Uuid::new_v4().into(),
            world_id: "main".to_owned(),
            sequence: 1,
            preview_scheduled_at: preview,
            scheduled_open_at: preview + Duration::hours(1),
            scheduled_end_at: preview + Duration::hours(2),
            expected_state_version: 0,
            actor_id: "operator".to_owned(),
            request_id: None,
            request_hash: RequestHash::new([1; 32]),
        }
    }

    #[test]
    fn create_auction_id_matches_public_uuid_versions_and_variant() {
        let mut command = command();
        assert_eq!(command.command_id.as_uuid().get_version_num(), 7);
        for version in 1..=5 {
            let uuid = format!("5b1c2a9e-1f0d-{version}c6b-9a7e-3d2f1e0c9b8a");
            command.auction_id = uuid.parse::<Uuid>().unwrap().into();
            assert!(command.validate(), "{uuid}");
        }
        for uuid in [
            "5b1c2a9e-1f0d-0c6b-9a7e-3d2f1e0c9b8a",
            "01976b9c-af94-7b04-89fa-5b49708c9faa",
            "5b1c2a9e-1f0d-4c6b-0a7e-3d2f1e0c9b8a",
        ] {
            command.auction_id = AuctionId::from_uuid(Uuid::parse_str(uuid).unwrap());
            assert!(!command.validate(), "{uuid}");
        }
    }

    #[test]
    fn create_auction_sequence_fits_the_public_safe_integer_range() {
        let mut command = command();
        for (sequence, valid) in
            [(1, true), (9_007_199_254_740_991, true), (9_007_199_254_740_992, false), (0, false)]
        {
            command.sequence = sequence;
            assert_eq!(command.validate(), valid, "{sequence}");
        }
    }
}
