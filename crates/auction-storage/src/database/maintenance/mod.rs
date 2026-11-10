//! 运维数据库能力的 owner 分组：建库、最小权限、重建、恢复预检、密钥轮换与保留清理。

pub(crate) mod bootstrap;
pub(crate) mod deploy_evidence;
pub(crate) mod external_recovery;
pub(crate) mod initialization;
pub(crate) mod key_rotation;
pub(crate) mod least_privilege;
pub(crate) mod rebuild;
pub(crate) mod recovery_preflight;
pub(crate) mod retention;
pub(crate) mod shutdown_window;
