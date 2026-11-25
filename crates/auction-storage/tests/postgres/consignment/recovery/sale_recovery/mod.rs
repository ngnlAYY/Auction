//! 7C-3 的 PG18 测试：Sale 已占代 g 后的撤回（追加 g+1 撤回代）、重列（g+1 重列代）与第二位买家购买、
//! 退款调用（6B 单写者）与 committed -> in_custody 回库边。
//!
//! 夹具：先走真实写端把 main 件取得并上架（挂单 available 版本 5，position 在库、处分代 1），再用一条事务
//! 写入“Sale 已占代 1、position committed、挂单止售为 held_off_market”的存量。这是 7A-2 / 7C-3 之后
//! 的真实状态，但挂单 sale_pending -> held_off_market 的数据库边尚无 owner，所以存量只能这样构造；
//! 构造本身不关任何触发器、不删任何约束，每条语句都通过同一套守卫。

mod fixture;
mod recall_edge;
pub(in crate::consignment) mod refund_call;
mod relist;
mod unknown_dispatch;
mod withdrawal;

pub(super) use fixture::{
    BUYER_3, Closure, HOLD, PRICE, World, closure_sql, command_id, database_time, id, seller,
    unknown_refund_sql,
};
