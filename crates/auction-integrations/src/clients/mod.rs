//! 外部服务客户端：E-Hentai 论坛、MoogleMail 及共用的请求门与指标。

pub mod ehentai;
pub mod mooglemail;

pub(crate) mod gate;
pub(crate) mod metrics;
