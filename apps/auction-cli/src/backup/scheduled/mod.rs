//! 定时备份装配：初始化、配置、持久运行、资源门禁、完成边界、保留清理与脱敏打卡。

mod completion;
mod disk;
mod filesystem;
mod heartbeat;
mod journal;
mod model;
mod offsite;
mod pipeline;
mod retention;
mod run;

pub(super) use run::run;
