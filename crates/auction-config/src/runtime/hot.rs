//! Worker 热读取的生产入口：读取 runtime TOML、完整校验候选文档，再提取热字段。
//!
//! 校验覆盖整份文档（含仅重启字段与其它角色参数），因此一个越界或冲突的候选不会通过
//! “只挑热字段”的捷径进入 Worker。读取沿用 runtime 文件的 owner、权限、类型和大小边界，
//! 错误只报告路径或固定文案，不回显 TOML。

use std::{path::Path, time::UNIX_EPOCH};

use auction_application::admin::{AdminRuntimeDocument, WorkerHotConfig};
use chrono::Utc;

use crate::{ConfigError, WorkerConfig};

use super::{read_runtime_file, validate_runtime_document};

/// runtime 文件的内容代次提示；只用于避免无变化时的重复解析，不参与配置身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeFileStamp {
    len: u64,
    modified_nanos: Option<u128>,
}

impl RuntimeFileStamp {
    /// 读取成功时记录的文件长度与修改时间；文件不可用时返回 `None`，由调用方决定告警。
    pub fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos());
        Some(Self { len: metadata.len(), modified_nanos })
    }

    /// 文件内容代次未变化时跳过重复读取；修改时间与长度任一变化都重新校验。
    pub fn unchanged_from(&self, previous: Option<&Self>) -> bool {
        match previous {
            Some(previous) => {
                self.len == previous.len && self.modified_nanos == previous.modified_nanos
            }
            None => false,
        }
    }
}

/// 读取并完整校验 runtime 文件，然后只提取热读取字段。
/// 未接管的参数组沿用 `current`，因此热重载不会把部署默认值写回正在运行的 Worker。
pub fn load_worker_hot_config(
    path: &Path,
    current: &WorkerHotConfig,
) -> Result<WorkerHotConfig, ConfigError> {
    let contents = read_runtime_file(path, None).map_err(map_read_error(path))?;
    let document: AdminRuntimeDocument = toml::from_str(&contents).map_err(|error| {
        let _ = error;
        ConfigError::RuntimeParse { path: path.to_owned() }
    })?;
    // 完整候选校验是热替换的前置条件；失败时调用方保留旧快照并告警。
    validate_runtime_document(&document, Utc::now()).map_err(|error| {
        ConfigError::InvalidRuntimeConfig { path: path.to_owned(), reason: error.to_string() }
    })?;
    Ok(WorkerHotConfig::apply_document(current, &document))
}

/// 启动快照中的热参数；没有 runtime 文件或尚未接管时仍从实际生效的 Worker 配置取值，
/// 让首次热读取与启动行为一致。
pub fn worker_hot_config(config: &WorkerConfig) -> WorkerHotConfig {
    WorkerHotConfig {
        mail: auction_application::admin::WorkerMailHotConfig {
            batch_size: config.mooglemail.batch_size,
            jitter_seconds: config.mooglemail.jitter_seconds,
            fetch_attempts: config.mooglemail.fetch_attempts,
            fetch_timeout_seconds: config.mooglemail.fetch_timeout_seconds,
            process_attempts: config.mooglemail.process_attempts,
            item_timeout_seconds: config.mooglemail.item_timeout_seconds,
            batch_timeout_seconds: config.mooglemail.batch_timeout_seconds,
            retry_base_seconds: config.mooglemail.retry_base_seconds,
            retry_maximum_seconds: config.mooglemail.retry_maximum_seconds,
            account_reply_window_seconds: config.mooglemail.account_reply_window_seconds,
            explanation_mail_window_seconds: config.mooglemail.explanation_mail_window_seconds,
            explanation_mail_limit: config.mooglemail.explanation_mail_limit,
            send_intervals: config.mooglemail.send_intervals,
        },
        forum: auction_application::admin::WorkerForumHotConfig {
            batch_size: config.forum.batch_size,
            poll_interval_seconds: config.forum.poll_interval_seconds,
            jitter_seconds: config.forum.jitter_seconds,
            item_timeout_seconds: config.forum.item_timeout_seconds,
        },
        delivery: auction_application::admin::WorkerDeliveryHotConfig {
            batch_limit: config.delivery.batch_limit,
            poll_interval_seconds: config.delivery.poll_interval_seconds,
            jitter_seconds: config.delivery.jitter_seconds,
            request_timeout_seconds: config.delivery.request_timeout_seconds,
            finalize_timeout_seconds: config.delivery.finalize_timeout_seconds,
        },
    }
}

fn map_read_error(path: &Path) -> impl FnOnce(super::RuntimeFileStoreError) -> ConfigError + '_ {
    move |error| match error {
        super::RuntimeFileStoreError::Parse => ConfigError::RuntimeParse { path: path.to_owned() },
        super::RuntimeFileStoreError::Io(message) => ConfigError::RuntimeRead {
            path: path.to_owned(),
            source: std::io::Error::other(message),
        },
        error => {
            ConfigError::InvalidRuntimeConfig { path: path.to_owned(), reason: error.to_string() }
        }
    }
}

/// 供测试构造修改时间代次；生产读取只经 `RuntimeFileStamp::read`。
#[cfg(test)]
fn stamp(len: u64, modified_nanos: Option<u128>) -> RuntimeFileStamp {
    RuntimeFileStamp { len, modified_nanos }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::{Path, PathBuf},
    };

    use auction_application::admin::{
        AdminRuntimeDocument, AdminRuntimeMailConfig, AdminRuntimeOperationsConfig, WorkerHotConfig,
    };
    use auction_application::delivery::DeliverySendIntervals;
    use uuid::Uuid;

    use self::parent::*;
    use crate::runtime::hot as parent;

    use crate::{DeliveryWorkerConfig, ForumSyncWorkerConfig, MoogleMailWorkerConfig};

    fn temp_dir() -> PathBuf {
        let directory = std::env::temp_dir().join(format!("auction-worker-hot-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        directory
    }

    fn write_document(directory: &Path, document: &AdminRuntimeDocument) -> PathBuf {
        let path = directory.join("admin.toml");
        fs::write(&path, toml::to_string(document).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        path
    }

    fn current() -> WorkerHotConfig {
        let config = WorkerConfig {
            mooglemail: MoogleMailWorkerConfig {
                batch_size: 100,
                send_intervals: DeliverySendIntervals { outbid_seconds: 2, ..Default::default() },
                ..Default::default()
            },
            forum: ForumSyncWorkerConfig { item_timeout_seconds: 240, ..Default::default() },
            delivery: DeliveryWorkerConfig { batch_limit: 100, ..Default::default() },
            ..Default::default()
        };
        worker_hot_config(&config)
    }

    fn managed_mail() -> AdminRuntimeDocument {
        let mut operations = AdminRuntimeOperationsConfig::default();
        let mail = operations.mail.get_or_insert_with(AdminRuntimeMailConfig::default);
        mail.managed = true;
        mail.batch_size = 7;
        mail.batch_timeout_seconds = 900;
        mail.send_intervals =
            DeliverySendIntervals { settlement_seconds: 45, ..Default::default() };
        // 仅重启字段与整份文档校验仍参与候选合法性，但不进入热快照。
        mail.schedule = "*/10 * * * *".to_owned();
        mail.maximum_pages = 5;
        AdminRuntimeDocument { operations: Some(operations), ..Default::default() }
    }

    #[test]
    fn valid_candidate_loads_hot_fields_and_ignores_restart_only_values() {
        let directory = temp_dir();
        let path = write_document(&directory, &managed_mail());
        let next = load_worker_hot_config(&path, &current()).unwrap();
        assert_eq!(next.mail.batch_size, 7);
        assert_eq!(next.mail.batch_timeout_seconds, 900);
        assert_eq!(next.mail.send_intervals.settlement_seconds, 45);
        // 未接管的论坛与 delivery 保持启动值；页数等重启字段没有热快照位置。
        assert_eq!(next.forum, current().forum);
        assert_eq!(next.delivery, current().delivery);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unmanaged_groups_keep_current_snapshot() {
        let directory = temp_dir();
        let mut document = managed_mail();
        document.operations.as_mut().unwrap().mail.as_mut().unwrap().managed = false;
        let path = write_document(&directory, &document);
        assert_eq!(load_worker_hot_config(&path, &current()).unwrap(), current());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_full_candidate_is_rejected_before_extraction() {
        let directory = temp_dir();
        let mut document = managed_mail();
        let mail = document.operations.as_mut().unwrap().mail.as_mut().unwrap();
        // batch_size 越界，属于热字段；完整候选校验必须在提取前拒绝。
        mail.batch_size = 0;
        let path = write_document(&directory, &document);
        assert!(matches!(
            load_worker_hot_config(&path, &current()),
            Err(ConfigError::InvalidRuntimeConfig { .. })
        ));

        // 非热字段无效同样拒绝，证明没有“只校验热字段”的捷径。
        let mut document = managed_mail();
        document.operations.as_mut().unwrap().mail.as_mut().unwrap().maximum_pages = 0;
        let path = write_document(&directory, &document);
        assert!(load_worker_hot_config(&path, &current()).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn non_runtime_toml_and_unknown_fields_are_rejected() {
        let directory = temp_dir();
        let path = directory.join("admin.toml");
        fs::write(&path, "operations = [\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(
            load_worker_hot_config(&path, &current()),
            Err(ConfigError::RuntimeParse { .. })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn file_stamp_change_detection_is_strict() {
        let first = stamp(10, Some(1));
        let same = stamp(10, Some(1));
        let longer = stamp(11, Some(1));
        let rewritten = stamp(10, Some(2));
        assert!(first.unchanged_from(Some(&same)));
        assert!(!first.unchanged_from(None));
        assert!(!longer.unchanged_from(Some(&first)));
        assert!(!rewritten.unchanged_from(Some(&first)));
    }

    #[test]
    fn startup_hot_snapshot_matches_worker_configuration() {
        let config = WorkerConfig {
            mooglemail: MoogleMailWorkerConfig {
                batch_size: 12,
                send_intervals: DeliverySendIntervals {
                    settlement_seconds: 30,
                    ..Default::default()
                },
                ..Default::default()
            },
            forum: ForumSyncWorkerConfig { item_timeout_seconds: 90, ..Default::default() },
            delivery: DeliveryWorkerConfig { request_timeout_seconds: 200, ..Default::default() },
            ..Default::default()
        };

        let hot = worker_hot_config(&config);
        assert_eq!(hot.mail.batch_size, 12);
        assert_eq!(hot.mail.send_intervals.settlement_seconds, 30);
        assert_eq!(hot.forum.item_timeout_seconds, 90);
        assert_eq!(hot.delivery.request_timeout_seconds, 200);
        // 仅重启字段不进入快照。
        assert_eq!(
            config.mooglemail.lease_duration_seconds,
            WorkerConfig::default().mooglemail.lease_duration_seconds
        );
    }
}
