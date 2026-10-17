//! PostgreSQL Public lot 读取适配器装配；列表、详情和行映射由子模块拥有，本模块只保留稳定导出。

mod detail;
mod list;
mod mapping;

pub(super) use detail::detail_postgres;
pub(super) use list::list_postgres;
