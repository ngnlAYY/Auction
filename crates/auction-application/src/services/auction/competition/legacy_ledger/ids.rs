//! 旧版确定性事实与授权 ID 的派生。

use auction_domain::CommandId;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 按已发布的确定性 ID 算法派生竞价事实或代理授权 ID；字节兼容、不可修改，由同目录
/// `tests.rs` 的 `deterministic_id_matches_frozen_wire_vectors` 冻结向量固定。
pub(in crate::services::auction::competition) fn deterministic_id(
    command_id: CommandId,
    namespace: &str,
    index: u32,
) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update(command_id.as_uuid().as_bytes());
    digest.update(index.to_string().as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
