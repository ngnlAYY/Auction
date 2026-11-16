//! Admin 结算退件案件的 HTTP 适配器：分页列表、详情、只读重新核验与处理命令。
//! handler 只做输入闭集、会话/CSRF、安全投影与类型化错误映射；准入、锁序、幂等与冻结资产都由
//! 存储层的命令事务负责，请求体只携带 `command_id`、案件版本与动作名，不能指定资产、金额或
//! 收件人。

mod boundary;
mod commands;
mod filters;
mod mapping;
mod reads;

pub(crate) use commands::{handle, recheck};
pub(crate) use reads::{detail, list};
