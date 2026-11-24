//! 实物交接历史的受控引用文本。

use uuid::Uuid;

/// 取得证明在 handoff 历史中的稳定、无可猜内容的引用。
pub(crate) fn custody_acquired_proof_ref(proof_id: Uuid) -> String {
    format!("consignment-take:{proof_id}")
}
