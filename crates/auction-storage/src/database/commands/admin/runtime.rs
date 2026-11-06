//! Admin runtime 配置命令账本适配器的模块装配；runtime 文件与数据库账本是独立持久化资源。
//! 本目录按分派、查询、事务 mutation、行映射和错误分类拆分，账本仍只由本适配器拥有。

mod dispatch;
mod errors;
mod mapping;
mod mutations;
mod queries;

pub use dispatch::DatabaseAdminRuntimeCommands;
