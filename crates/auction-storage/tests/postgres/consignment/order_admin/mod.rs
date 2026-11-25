//! 7E-2 的 PG18 测试：Admin 订单 resolve 的动作闭集、退款类处置只经 6B 单写者、相反付款拒绝，
//! 补寄只开新轮且上一轮须已明确封闭，recheck 只排持久工作，以及账本与处置表的数据库守卫。
//!
//! 只测核心规则：金额与资金（退款一次、相反付款零资金痕迹）、命令幂等（同 ID 重放、异请求冲突）、
//! 资产外写（补寄不自动开轮）、安全边界（失败关闭、数据库最后一道门）。

mod effects;
mod evidence;
mod fixture;
mod redispatch;
mod resolve;
mod schema;

pub(super) use fixture::{ADMIN, command, count, ledger_of, rejection, resolve_input, service};
