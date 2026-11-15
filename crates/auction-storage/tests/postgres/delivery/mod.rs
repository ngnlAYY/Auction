//! 投递 outbox、载荷列、出站分类开关、玩家通知预检退役、资产冲突排除、发送围栏收口与
//! 提交后唤醒的 PostgreSQL 18 用例。

mod account_notices;
mod disposition;
mod fence;
mod mail_commit_wakeup;
mod notice_conflicts;
mod payload_columns;
mod recipient_renames;
mod send_classes;
