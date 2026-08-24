//! PostgreSQL Public lot 读取适配器装配。
//!
//! 列表、详情和数据库行映射分别由子模块拥有；本模块只负责稳定导出 PostgreSQL
//! 读取入口，避免后端查询与映射逻辑重新聚合到单一文件。

mod detail;
mod list;
mod mapping;

pub(super) use detail::detail_postgres;
pub(super) use list::list_postgres;
