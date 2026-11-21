//! 寄售装备来源脱敏样本与只监听回环的响应替身。

mod scripted;

pub use scripted::{Chunk, EquipmentDouble, RequestCheck, ResponseScript};

/// 成功样本所用的虚构装备编号。
pub const FIXTURE_EID: &str = "424242";
/// 成功样本所用的虚构读取凭证。
pub const FIXTURE_KEY: &str = "decaf00000";
/// 成功样本所用的虚构持有人。
pub const FIXTURE_OWNER: &str = "FixtureOwner";

/// 保留实际字段、类型和值形态的成功样本，替换 EID、key、持有人名称和 UID。
pub const EQUIPMENT: &[u8] = include_bytes!("fixtures/equip.json");
/// 错 key 时捕获的错误形态。
pub const NOT_FOUND: &[u8] = include_bytes!("fixtures/badkey.json");
/// 缺少 query 参数时捕获的错误形态。
pub const MISSING_PARAMETER: &[u8] = include_bytes!("fixtures/missing.json");
