//! 派发轮次、尝试、证据与首派的 PG18 测试：首轮键与冻结身份、起写先于 POST 且每投递只一次、
//! 崩溃重启零第二次 POST、跨来源唯一 Sent MID、证据只追加。

pub(in crate::consignment) mod carrier_funding;
mod cash_gate;
pub(in crate::consignment) mod cash_rounds;
mod evidence;
mod executor;
mod expired_sweep;
mod first_round;
pub(in crate::consignment) mod fixture;
mod generic_guard;
mod order_recheck;
mod recheck_consumer;
mod recipient_gate;
mod redispatch;
mod redispatch_source;
mod round_lease;
mod sent_close;
mod withdrawal_dispatch;
mod work_claim;
pub(in crate::consignment) mod worker_chain;
mod write_start;
mod write_start_guards;
