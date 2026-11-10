//! 与后端通信的通用边界：同源请求、URL query、wire 谓词与稳定错误分类。

pub mod commands;
pub mod errors;
pub mod query;
pub mod transport;
pub mod wire;
