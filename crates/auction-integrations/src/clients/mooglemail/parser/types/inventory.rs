//! Armory organize 页面的完整装备身份快照；只保存外部装备 ID，不保存名称或标签。

use chrono::{DateTime, Utc};

use self::parent::*;
use super as parent;
use crate::clients::mooglemail::protocol::MoogleMailInventoryCategory;

/// 一个 world、一个已批准分类的完整库存身份集合。
///
/// 页面结构、计数与行身份全部通过后才产生快照；空集合是有效的“该分类没有可整理装备”，
/// 但不是“整个 Armory 没有该装备”的证明，调用方必须合并全部分类后再比较。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailInventorySnapshot {
    pub world: MoogleMailWorld,
    pub category: MoogleMailInventoryCategory,
    /// 按字符串升序、无重复的外部装备 ID。
    pub equipment_ids: Vec<String>,
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailInventorySnapshot {
    pub fn validate(&self) -> bool {
        self.observed_at.timestamp_nanos_opt().is_some()
            && self.equipment_ids.len() <= 100_000
            && self
                .equipment_ids
                .iter()
                .all(|id| auction_application::valid_external_positive_id(id))
            && self.equipment_ids.windows(2).all(|ids| ids[0] < ids[1])
    }
}
