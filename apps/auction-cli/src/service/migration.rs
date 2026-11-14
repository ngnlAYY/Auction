//! 启停流程中的迁移 owner 加载、迁移执行与服务账号解析。

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;

use super::lifecycle::MigrationOutcome;
use super::systemd::UnitState;
use crate::config_dir::Deployment;
use crate::receipt::MigrationRecord;

pub(super) fn load_owner(
    owner: &crate::cli::OwnerSecretsArg,
) -> anyhow::Result<auction_config::PostgresOwnerSecrets> {
    crate::database::load_owner_secrets(&owner.owner_secrets)
}

pub(super) async fn migrate(
    deployment: &Deployment,
    owner: &auction_config::PostgresOwnerSecrets,
) -> anyhow::Result<MigrationOutcome> {
    let report = crate::database::apply_migrations(&deployment.worker, owner).await?;
    Ok(MigrationOutcome {
        record: MigrationRecord {
            before_version: report.before_version,
            after_version: report.after_version,
            expected_version: report.expected_version,
            applied: report.applied,
        },
        report: serde_json::to_value(&report)?,
    })
}

/// Admin 以原子替换保存 runtime：文件与可写父目录必须归 Admin unit 的固定服务账号。
pub(super) fn require_runtime_owner(path: &Path, state: &UnitState) -> anyhow::Result<()> {
    if state.dynamic_user {
        anyhow::bail!("Admin runtime 需要固定服务账号；请关闭 Admin unit 的 DynamicUser");
    }
    let uid = service_uid(&state.user)?;
    let file = std::fs::symlink_metadata(path).context("读取 Admin runtime 文件失败")?;
    let parent = path.parent().context("Admin runtime 缺少父目录")?;
    let directory = std::fs::symlink_metadata(parent).context("读取 Admin runtime 父目录失败")?;
    if file.uid() != uid
        || directory.uid() != uid
        || directory.permissions().mode() & 0o300 != 0o300
    {
        anyhow::bail!(
            "Admin runtime 文件与父目录必须归 Admin unit 账号（UID {uid}）所有且父目录可写：{}",
            path.display()
        );
    }
    Ok(())
}

/// systemd 的空 User 表示 root；用户名通过 NSS 解析，不用调用者 UID 推断。
pub(crate) fn service_uid(user: &str) -> anyhow::Result<u32> {
    if user.is_empty() {
        return Ok(0);
    }
    if let Ok(uid) = user.parse::<u32>() {
        return Ok(uid);
    }
    let name = std::ffi::CString::new(user).context("unit User 含非法字符")?;
    let mut buffer = vec![0_u8; 4096];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY：name 以 NUL 结尾；record、buffer、result 在调用期间有效且互不重叠。
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            anyhow::bail!("无法解析 unit User：{user}");
        }
        // SAFETY：getpwnam_r 成功且 result 非空，record 已完整初始化。
        return Ok(unsafe { record.assume_init().pw_uid });
    }
}
