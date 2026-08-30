//! Admin 装备来源与拍品录入端口。
//!
//! 外部来源必须先在数据库锁之外被规范化；进入 owner 事务的结构不包含原始 URL、canonical
//! JSON 或明文装备 key，只携带公开装备事实、来源摘要和加密信封。

use std::{collections::BTreeMap, fmt, future::Future};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{
    CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, EquipmentKeyEnvelopeError,
};

use super::{ApplicationFuture, CompetitionTransactionError, RequestHash};

mod commands;
mod common;
mod equipment;
mod ports;
mod records;
mod source;

pub use commands::AdminIntakeCommand;
pub use common::{AdminIntakeError, AdminIntakePolicy, aggregate_id};
pub(super) use common::{decimal_identity, valid_optional_text, valid_text};
pub(super) use equipment::CanonicalEquipmentDigest;
pub use equipment::{AdminIntakeSourceKind, CanonicalEquipment, CanonicalEquipmentStat};
pub use ports::{
    AdminIntakePort, AdminIntakeSourceReader, AdminIntakeTransaction, EquipmentKeySealerPort,
};
pub use records::{AdminIntakeRecord, AdminIntakeResult, AdminIntakeWrite};
pub use source::AdminIntakeSourceSnapshot;
