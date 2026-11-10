//! Operations alert 水位读取与 signal 观察写入的模块装配。

mod disposition;
mod observe;
mod reads;

pub(super) use disposition::apply_read_mode;
pub(super) use observe::{observe_host_postgres, observe_postgres};
pub(super) use reads::snapshot_postgres;
