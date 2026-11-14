//! Admin 装备来源与拍品录入端口。
//! 外部来源必须先在数据库锁之外被规范化；进入 owner 事务的结构不包含原始 URL、canonical
//! JSON 或明文装备 key，只携带公开装备事实、来源摘要和加密信封。

mod commands;
mod common;
mod equipment;
mod ports;
mod records;
mod source;

pub use commands::AdminIntakeCommand;
pub use common::{AdminIntakeError, AdminIntakePolicy, aggregate_id};
pub use equipment::{AdminIntakeSourceKind, CanonicalEquipment, CanonicalEquipmentStat};
pub use ports::{
    AdminIntakePort, AdminIntakeSourceReader, AdminIntakeTransaction, EquipmentKeySealerPort,
};
pub use records::{AdminIntakeRecord, AdminIntakeResult, AdminIntakeWrite};
pub use source::AdminIntakeSourceSnapshot;
