//! 7C-4b 灭失确认测试的共用夹具：管理员 / 系统命令、证据追加、买方侧事实快照与占用读数。
//!
//! 夹具沿 7C-3：真实写端取得并上架，再写入“已售、止售、Sale 已封闭”的存量，不关任何触发器；
//! 场景与断言见 `position`（头号验收与从未成交）和 `current_generation`（权责分流的反例）。

mod current_generation;
mod fixture;
mod owned_link;
mod position;
mod reconcile_binding;

pub(super) use fixture::{
    admin_command_id, all_command_ids, buyer_side_snapshot, claim_released, confirm, loss_rows,
    prior_receipts, record_proof, rejection,
};
