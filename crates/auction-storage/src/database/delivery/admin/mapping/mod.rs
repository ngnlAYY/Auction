//! delivery Admin 安全投影与 payload metadata 映射。

mod evidence;
mod rows;
mod status;
mod validation;

pub(crate) use rows::{decode_result_payload, legacy_result_payload, snapshot, stored_snapshot};
pub(crate) use validation::valid_snapshot;

#[cfg(test)]
mod tests;
