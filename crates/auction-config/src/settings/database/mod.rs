//! 数据库连接分档的非敏感配置；连接数包含长期所有权，会话参数只在重启建连时应用。

mod model;
mod validation;

pub use model::*;
