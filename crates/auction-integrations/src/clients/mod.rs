//! 外部服务客户端：E-Hentai 论坛、MoogleMail 及共用的请求门、优先锁、owner 探测与指标。

pub mod ehentai;
pub mod mooglemail;

pub(crate) mod gate;
pub(crate) mod metrics;
pub(crate) mod operation_lock;
pub(crate) mod owner_probe;
