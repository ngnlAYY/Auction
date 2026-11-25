//! 一口价挂单（寄售与非寄售）的 PostgreSQL 实现入口：命令仲裁与事实写入、挂单投影、
//! 资格与可见性谓词、解析工作与全局许可、读取。各子模块的职责见其模块文档。

pub(crate) mod commands;
pub(crate) mod contracts;
pub(crate) mod custody;
pub(crate) mod deposit;
pub(crate) mod dispatch;
pub(super) mod eligibility;
pub(crate) mod listings;
pub(crate) mod lookup;
pub(crate) mod purchase;
pub(crate) mod reads;
pub(crate) mod recovery;
pub(crate) mod refusal;
pub(crate) mod returns;
pub(crate) mod rights;
