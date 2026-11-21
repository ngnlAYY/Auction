//! PostgreSQL 竞价事务子模块装配；事务边界与重试由父模块的 `PostgresCompetitionTransaction` 持有，
//! 命令账本、锁内快照、代理/lot 写入、审计和 realtime 事件由子模块拥有。

mod audit;
mod eligibility;
mod facts;
mod ledger;
mod plan;
mod proxy;
mod realtime;
mod snapshot;
