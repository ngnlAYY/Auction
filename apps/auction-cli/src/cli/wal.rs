//! WAL 归档与暂存上传命令的参数定义；archive 只写本地暂存，upload 只读暂存并写 R2。

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum WalCommand {
    /// PostgreSQL archive_command 入口：zstd 压缩 + age 公钥加密后 fsync 到本地暂存目录。
    Archive {
        /// 与上传器共享的暂存目录；必须已存在、绝对路径、禁止 other 写入。
        #[arg(long, value_name = "DIR")]
        staging_dir: PathBuf,
        /// age X25519 收件人公钥；须与 [backup.offsite] age_recipient 相同，私钥离线保存。
        #[arg(long, value_name = "AGE1...")]
        recipient: String,
        /// PostgreSQL 传入的 WAL 文件（%p）；重试时必须是同一文件。
        #[arg(value_name = "SOURCE")]
        source: PathBuf,
        /// PostgreSQL 传入的文件名（%f）；必须等于 SOURCE 的文件名。
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// 把暂存 WAL 与每周基础备份上传到 R2 的独立前缀，校验后按保留策略删除本地副本。
    Upload {
        /// 归档命令写入的同一暂存目录。
        #[arg(long, value_name = "DIR")]
        staging_dir: PathBuf,
        /// 单次运行的总预算（秒）；到期保留未处理文件并退出非零。
        #[arg(long, value_name = "SECONDS", default_value_t = 300)]
        budget_seconds: u64,
    },
    /// 每周物理基础备份：pg_basebackup 流式 zstd 压缩 + age 公钥加密后写入暂存目录。
    BaseBackup {
        /// 与上传器共享的暂存目录；必须已存在、绝对路径、禁止 other 写入。
        #[arg(long, value_name = "DIR")]
        staging_dir: PathBuf,
        /// age X25519 收件人公钥；须与 [backup.offsite] age_recipient 相同，私钥离线保存。
        #[arg(long, value_name = "AGE1...")]
        recipient: String,
    },
}
