//! 购买存储事务的 PG18 测试：一次提交全部事实与同 ID 重放、命令冲突、两买家并发、
//! 与改价 / 止售交换提交顺序、图写入末项失败整笔回滚、资金预检的确定拒绝与未知读取，以及买家处罚阻断、严格现金缺边界失败关闭、预检不在持锁事务内调用。

mod atomicity;
mod commands;
mod dispatch_gate;
mod fixture;
mod funding_guard;
mod guards;
mod orders;
mod owned_receipts;
mod race;
mod rights;
