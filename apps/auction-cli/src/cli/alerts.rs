//! 主机侧运维告警报告命令的参数定义；只采集主机事实，不执行备份、上传或数据库写入。

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum AlertCommand {
    /// 读取备份目录水位、定时备份终态回执与 WAL 暂存积压，并写入对应的运维告警来源。
    ///
    /// 只写 `backup` 与 `wal` 两个主机侧来源；Worker 的其它来源不受影响。重复运行是幂等的：
    /// 同一 `kind`/`scope` 命中同一 signal，不会产生第二条告警或重置下一次通知时间。
    Report {
        /// 定时备份的输出目录；报告读取该目录所在文件系统的可用块。
        #[arg(long, value_name = "DIR")]
        backup_output_dir: PathBuf,
        /// 定时备份的状态目录；报告读取 `<state-dir>/receipts/` 下最新的终态回执。
        #[arg(long, value_name = "DIR")]
        backup_state_dir: PathBuf,
        /// WAL 暂存目录；省略时不报告 `wal` 来源，已有 `wal` 告警保持原状。
        #[arg(long, value_name = "DIR")]
        wal_staging_dir: Option<PathBuf>,
        /// 决策 31：运维留存恢复演练回执的目录（成功的 `backup restore` 回执或结论为
        /// `passed` 的迁移演练回执）。超过 35 天没有成功演练时产生 `restore_drill_overdue`；
        /// 省略时不检查演练。
        #[arg(long, value_name = "DIR")]
        restore_drill_dir: Option<PathBuf>,
    },
}
