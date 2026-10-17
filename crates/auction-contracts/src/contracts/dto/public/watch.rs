//! 当前会话用户的装备标记 HTTP 契约；取消记录保留正版本 tombstone。

use super::*;

/// 单个装备的权威标记状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicLotWatchResponse {
    pub lot_id: String,
    pub watched: bool,
    pub channel: String,
    pub version: i64,
}

/// 指定拍卖内当前用户的完整标记集合。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicLotWatchesResponse {
    pub user_id: String,
    pub auction_id: String,
    pub watches: Vec<PublicLotWatchResponse>,
}

/// CAS 写入；请求结果未知时必须保留原命令及完整负载重试。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSetLotWatchRequest {
    pub command_id: String,
    pub watched: bool,
    pub channel: String,
    pub expected_version: i64,
}

/// 可重放的原命令结果；重放后客户端重新读取权威集合。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicSetLotWatchResponse {
    pub user_id: String,
    pub auction_id: String,
    pub watch: PublicLotWatchResponse,
}
