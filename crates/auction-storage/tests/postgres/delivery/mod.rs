//! 投递 outbox、载荷列、出站分类开关、玩家通知预检退役与提交后唤醒的 PostgreSQL 18 用例。

mod account_notices;
mod disposition;
mod mail_commit_wakeup;
mod payload_columns;
mod recipient_renames;
mod send_classes;
