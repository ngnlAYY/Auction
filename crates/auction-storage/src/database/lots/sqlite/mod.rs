//! SQLite Lite Public lot 读取适配器装配。
//!
//! Lite 查询显式遵循 SQLite 能力边界；列表、详情和行映射分别由子模块拥有，本模块只负责
//! 稳定导出读取入口，不把 PostgreSQL 锁或完整档字段伪装成等价能力。

mod detail;
mod list;
mod mapping;

pub(super) use detail::detail_sqlite;
pub(super) use list::list_sqlite;
