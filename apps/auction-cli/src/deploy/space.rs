//! 停机前的磁盘空间门禁：估算停机之后才写入的停机备份（只有 deploy）、旧程序保留与安装临时
//! 文件，按文件系统合并后与非特权可用空间比较，并沿用 Worker 配置 `[backup]` 的余量阈值；不足
//! 就在停机前拒绝。deploy 与 rollback 共用。
//!
//! 估算依据：停机备份取 `pg_database_size`（custom 格式经压缩且不含索引数据，是偏保守的上界）；
//! 旧程序保留取安装目录当前四个程序的字节合计；安装取最大的单个待安装程序（同目录临时文件）加
//! 各程序新旧大小的正增量合计（替换后旧文件即释放，正增量是净占用）。估算只是检查时刻的快照，
//! 不预留空间，也不替代停机后各步骤自身的失败处理。文件系统按设备号合并：btrfs 子卷等共享
//! 存储池的布局各有设备号，合计需求会被低估。

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_config::BackupConfig;
use serde::Serialize;

use super::artifact::REQUIRED_BINARIES;
use crate::support::files::{DiskUsage, disk_usage};

/// 停机后某一类写入落在 `path` 所在文件系统上的字节数。
struct Demand {
    purpose: &'static str,
    path: PathBuf,
    bytes: u64,
}

/// 同一文件系统（设备号相同）上合并后的需求与判定。
#[derive(Debug, Serialize)]
struct FilesystemCheck {
    paths: Vec<PathBuf>,
    purposes: Vec<&'static str>,
    required_bytes: u64,
    available_bytes: u64,
    total_bytes: u64,
    passed: bool,
}

/// 余量阈值：扣除需求后的可用空间仍须同时满足字节与比例下限。
#[derive(Debug, Clone, Copy)]
struct Margin {
    min_free_bytes: u64,
    min_free_percent: u8,
}

/// 估算停机后写入所需空间并逐个文件系统判定；返回写入报告的估算与判定，任一文件系统不足即
/// 失败（错误同样给出所需与可用字节），调用方必须在停机前调用。`backup` 是停机备份的目录与
/// 库大小上界（rollback 不备份时为 None），`incoming` 是待安装程序按名称的字节数。
pub(super) fn require_space(
    backup: Option<(&Path, u64)>,
    state_dir: &Path,
    install_dir: &Path,
    incoming: &[(&str, u64)],
    config: &BackupConfig,
) -> anyhow::Result<serde_json::Value> {
    let mut sizes = Vec::with_capacity(REQUIRED_BINARIES.len());
    for name in REQUIRED_BINARIES {
        let path = install_dir.join(name);
        // 与保留旧程序的要求一致：必须是非符号链接、非空的普通文件，否则停机后保留必然失败。
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("读取当前安装程序失败：{}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
            anyhow::bail!("当前安装程序不是可保留的普通文件：{}", path.display());
        }
        let new = incoming
            .iter()
            .find(|(incoming, _)| *incoming == name)
            .map(|(_, bytes)| *bytes)
            .with_context(|| format!("待安装程序缺少 {name}"))?;
        sizes.push((new, metadata.len()));
    }
    let retain_bytes = sizes.iter().fold(0_u64, |total, (_, old)| total.saturating_add(*old));
    let install_bytes = install_bytes(&sizes);
    let mut demands = Vec::with_capacity(3);
    if let Some((backup_dir, database_size_bytes)) = backup {
        demands.push(Demand {
            purpose: "shutdown_backup",
            path: backup_dir.to_path_buf(),
            bytes: database_size_bytes,
        });
    }
    demands.push(Demand {
        purpose: "retained_binaries",
        path: state_dir.to_path_buf(),
        bytes: retain_bytes,
    });
    demands.push(Demand {
        purpose: "install_temporary",
        path: install_dir.to_path_buf(),
        bytes: install_bytes,
    });
    let mut located = Vec::with_capacity(demands.len());
    let mut usage = BTreeMap::new();
    for demand in demands {
        let device = fs::metadata(&demand.path)
            .with_context(|| format!("读取目录所在文件系统失败：{}", demand.path.display()))?
            .dev();
        if let Entry::Vacant(entry) = usage.entry(device) {
            entry.insert(
                disk_usage(&demand.path)
                    .with_context(|| format!("读取磁盘水位失败：{}", demand.path.display()))?,
            );
        }
        located.push((device, demand));
    }
    let margin =
        Margin { min_free_bytes: config.min_free_bytes, min_free_percent: config.min_free_percent };
    let filesystems = evaluate(located, &usage, margin);
    let shortages = filesystems
        .iter()
        .filter(|filesystem| !filesystem.passed)
        .map(|filesystem| {
            format!(
                "{}（{}）需要 {} 字节，可用 {} 字节",
                filesystem
                    .paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、"),
                filesystem.purposes.join("+"),
                filesystem.required_bytes,
                filesystem.available_bytes
            )
        })
        .collect::<Vec<_>>();
    if !shortages.is_empty() {
        anyhow::bail!(
            "停机后写入所需磁盘空间不足：{}；扣除后还须保留至少 {} 字节且不少于总量的 {}%\
             （Worker 配置 [backup] 的 min_free_bytes/min_free_percent）。清理或扩容后重试，\
             尚未停止任何服务",
            shortages.join("；"),
            margin.min_free_bytes,
            margin.min_free_percent
        );
    }
    Ok(serde_json::json!({
        "estimate": {
            "shutdown_backup_bytes": backup.map(|(_, bytes)| bytes),
            "retained_binaries_bytes": retain_bytes,
            "install_temporary_bytes": install_bytes,
            "basis": {
                "shutdown_backup": "pg_database_size，custom 格式备份的保守上界",
                "retained_binaries": "安装目录当前四个程序的字节合计",
                "install_temporary": "最大单个待安装程序加各程序新旧大小的正增量合计",
            },
        },
        "min_free_bytes": margin.min_free_bytes,
        "min_free_percent": margin.min_free_percent,
        "filesystems": filesystems,
    }))
}

/// 安装逐个以同目录临时文件替换程序：峰值占用不超过最大的单个新程序加全部正增量。
fn install_bytes(sizes: &[(u64, u64)]) -> u64 {
    let largest = sizes.iter().map(|(new, _)| *new).max().unwrap_or_default();
    sizes.iter().fold(largest, |total, (new, old)| total.saturating_add(new.saturating_sub(*old)))
}

/// 按设备号合并需求并判定；比例用整数比较，避免浮点取整放宽门禁。
fn evaluate(
    demands: Vec<(u64, Demand)>,
    usage: &BTreeMap<u64, DiskUsage>,
    margin: Margin,
) -> Vec<FilesystemCheck> {
    let mut merged: BTreeMap<u64, FilesystemCheck> = BTreeMap::new();
    for (device, demand) in demands {
        let measured = usage.get(&device).copied();
        let entry = merged.entry(device).or_insert_with(|| FilesystemCheck {
            paths: Vec::new(),
            purposes: Vec::new(),
            required_bytes: 0,
            available_bytes: measured.map_or(0, |usage| usage.available_bytes),
            total_bytes: measured.map_or(0, |usage| usage.total_bytes),
            passed: false,
        });
        if !entry.paths.contains(&demand.path) {
            entry.paths.push(demand.path);
        }
        entry.purposes.push(demand.purpose);
        entry.required_bytes = entry.required_bytes.saturating_add(demand.bytes);
    }
    merged
        .into_values()
        .map(|mut filesystem| {
            let remaining = filesystem.available_bytes.checked_sub(filesystem.required_bytes);
            filesystem.passed = filesystem.total_bytes != 0
                && remaining.is_some_and(|remaining| {
                    remaining >= margin.min_free_bytes
                        && u128::from(remaining) * 100
                            >= u128::from(filesystem.total_bytes)
                                * u128::from(margin.min_free_percent)
                });
            filesystem
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use auction_config::BackupConfig;

    use super::{Demand, Margin, evaluate, install_bytes, require_space};
    use crate::deploy::artifact::REQUIRED_BINARIES;
    use crate::support::files::DiskUsage;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-deploy-space-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("建立临时目录");
            Self(root)
        }

        fn directory(&self, name: &str) -> PathBuf {
            let directory = self.0.join(name);
            fs::create_dir_all(&directory).expect("建立目录");
            directory
        }

        /// 安装目录与当前四个旧程序。
        fn install(&self) -> PathBuf {
            let install = self.directory("install");
            for name in REQUIRED_BINARIES {
                fs::write(install.join(name), format!("old-{name}")).expect("写入旧程序");
            }
            install
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 待安装程序按名称的字节数，与制品或回滚记录的投影同形。
    fn incoming() -> Vec<(&'static str, u64)> {
        REQUIRED_BINARIES
            .into_iter()
            .map(|name| (name, format!("new-program-{name}").len() as u64))
            .collect()
    }

    fn unbounded() -> BackupConfig {
        BackupConfig { min_free_bytes: 0, min_free_percent: 0, ..BackupConfig::default() }
    }

    fn demand(purpose: &'static str, path: &str, bytes: u64) -> Demand {
        Demand { purpose, path: PathBuf::from(path), bytes }
    }

    fn usage(entries: &[(u64, u64, u64)]) -> BTreeMap<u64, DiskUsage> {
        entries
            .iter()
            .map(|(device, total_bytes, available_bytes)| {
                (
                    *device,
                    DiskUsage { total_bytes: *total_bytes, available_bytes: *available_bytes },
                )
            })
            .collect()
    }

    const MARGIN: Margin = Margin { min_free_bytes: 500, min_free_percent: 1 };

    /// 同一临时文件系统上的三类需求合并为一项；需求超过可用空间或安装程序缺失时在停机前拒绝。
    #[test]
    fn require_space_measures_real_directories() {
        let root = TempRoot::new();
        let install = root.install();
        let backup = root.directory("backups");
        let state = root.directory("state");
        let config = unbounded();
        let incoming = incoming();

        let report = require_space(Some((&backup, 1)), &state, &install, &incoming, &config)
            .expect("小需求通过");
        let filesystems = report["filesystems"].as_array().expect("文件系统列表");
        assert_eq!(filesystems.len(), 1, "同一文件系统的需求合并");
        let retained: u64 =
            REQUIRED_BINARIES.iter().map(|name| format!("old-{name}").len() as u64).sum();
        assert_eq!(report["estimate"]["retained_binaries_bytes"], retained);
        assert_eq!(report["estimate"]["shutdown_backup_bytes"], 1);
        assert_eq!(filesystems[0]["purposes"].as_array().expect("用途").len(), 3);

        let error =
            require_space(Some((&backup, u64::MAX / 4)), &state, &install, &incoming, &config)
                .expect_err("超大需求必须拒绝");
        let text = format!("{error:#}");
        assert!(
            text.contains("需要") && text.contains("可用") && text.contains("尚未停止任何服务")
        );

        fs::remove_file(install.join("auction-cli")).expect("移除安装程序");
        assert!(require_space(Some((&backup, 1)), &state, &install, &incoming, &config).is_err());
    }

    /// 与保留旧程序的要求一致：零字节的当前程序在停机前就拒绝，不等到停机后保留时才失败。
    #[test]
    fn empty_installed_program_is_rejected_before_shutdown() {
        let root = TempRoot::new();
        let install = root.install();
        fs::write(install.join("auction-worker"), b"").expect("截断程序");
        let error = require_space(
            Some((&root.directory("backups"), 1)),
            &root.directory("state"),
            &install,
            &incoming(),
            &unbounded(),
        )
        .expect_err("零字节程序必须拒绝");
        assert!(format!("{error:#}").contains("auction-worker"));
    }

    /// rollback 不做停机备份：只有保留与安装两类需求；待安装程序缺项同样拒绝。
    #[test]
    fn rollback_space_has_no_backup_demand() {
        let root = TempRoot::new();
        let install = root.install();
        let state = root.directory("state");
        let report =
            require_space(None, &state, &install, &incoming(), &unbounded()).expect("回退需求通过");
        assert!(report["estimate"]["shutdown_backup_bytes"].is_null());
        let purposes = report["filesystems"][0]["purposes"].as_array().expect("用途").clone();
        assert_eq!(purposes, vec!["retained_binaries", "install_temporary"]);
        let missing = &incoming()[..3];
        assert!(require_space(None, &state, &install, missing, &unbounded()).is_err());
    }

    #[test]
    fn install_estimate_is_largest_binary_plus_growth() {
        assert_eq!(install_bytes(&[(10, 12), (30, 25), (20, 20), (5, 1)]), 30 + 5 + 4);
        assert_eq!(install_bytes(&[]), 0);
    }

    #[test]
    fn demands_on_the_same_filesystem_are_added_together() {
        let checks = evaluate(
            vec![
                (1, demand("shutdown_backup", "/var/backups/auction", 400)),
                (1, demand("retained_binaries", "/var/lib/auction/deployment", 200)),
                (2, demand("install_temporary", "/opt/auction/bin", 10)),
            ],
            &usage(&[(1, 10_000, 1_000), (2, 1_000, 600)]),
            MARGIN,
        );
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].required_bytes, 600);
        assert_eq!(checks[0].paths.len(), 2);
        // 单看任一需求都够用，合并后剩余 400 低于 500 字节余量。
        assert!(!checks[0].passed);
        assert_eq!(checks[1].required_bytes, 10);
        assert!(checks[1].passed);
    }

    #[test]
    fn margin_requires_both_bytes_and_percent() {
        let percent = Margin { min_free_bytes: 0, min_free_percent: 10 };
        let check = |required| {
            evaluate(
                vec![(7, demand("shutdown_backup", "/var/backups/auction", required))],
                &usage(&[(7, 10_000, 1_100)]),
                percent,
            )
            .remove(0)
            .passed
        };
        assert!(check(100), "剩余恰好 10% 时通过");
        assert!(!check(101), "剩余低于 10% 时拒绝");
        let exceeded = evaluate(
            vec![(7, demand("shutdown_backup", "/var/backups/auction", 2_000))],
            &usage(&[(7, 10_000, 1_100)]),
            Margin { min_free_bytes: 0, min_free_percent: 0 },
        );
        assert!(!exceeded[0].passed, "需求超过可用空间时拒绝");
        let unknown = evaluate(
            vec![(9, demand("install_temporary", "/opt/auction/bin", 1))],
            &usage(&[]),
            Margin { min_free_bytes: 0, min_free_percent: 0 },
        );
        assert!(!unknown[0].passed, "没有水位数据时失败关闭");
    }
}
