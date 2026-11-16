//! 旧程序保留、逐个原子替换与回滚记录：安装前把将被替换的程序复制到状态目录（见 `record`），
//! 每个程序以同目录临时文件 + rename 替换，失败不留下半写程序。deploy 替换四个程序；rollback
//! 只替换三个运行角色，安装目录中的 auction-cli 保持不变。多个程序整体不是原子的：安装中途
//! 失败时，自动把本次已替换的程序从刚保留的回滚记录换回（见 `restore`），换回也失败时安装目录
//! 才可能新旧混合。安装目录与程序的信任边界、root:root 0755 属主与描述符固定见 `trust`；回滚
//! 记录中的保留程序只经记录目录的固定描述符读取（[`RecordDirectory`]）。

use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::Context;

use super::artifact::Artifact;
use crate::app::progress;
use crate::support::digest::sha256_path;

mod record;
mod restore;
mod trust;

#[cfg(test)]
pub(crate) use record::{FORMAT, LEGACY_FORMAT};
pub(crate) use record::{
    RECORD_FILE, ROLE_BINARIES, RecordDirectory, RetainRequest, RetainedBinary, RetainedIdentity,
    RollbackRecord, latest, load, record_dir, record_projection, retain,
};
pub(crate) use trust::{RunAccount, require_install_dir};

use self::record::copy_contents;
use self::trust::PinnedInstallDir;

/// 制品或旧程序安装失败；`restored` 为真表示本次替换范围内的程序已复核为换回所用回滚记录中的
/// 程序（deploy 是部署前的四个程序，rollback 是回退前的三个角色）。
#[derive(Debug)]
pub(crate) struct InstallArtifactFailure {
    pub(crate) error: anyhow::Error,
    pub(crate) restored: bool,
}

/// 安装制品中的四个程序；临时文件与目标同目录，摘要、属主与权限复核后才 rename 覆盖。
///
/// 不变量：四个程序整体不是原子的。任一程序失败时，本次已经 rename 的程序（含 rename 之后
/// 复核失败的那一个）立即从 `retained` 换回，再按记录复核全部四个程序；这发生在迁移之前，
/// 不启动服务。换回失败时不重试，错误列出各程序的当前摘要，交由人工核对。
pub(crate) fn install_artifact(
    artifact: &Artifact,
    install_dir: &Path,
    retained: &RollbackRecord,
    record_directory: &RecordDirectory,
) -> Result<(), InstallArtifactFailure> {
    let programs = artifact.binaries.iter().map(|binary| Program {
        name: binary.name,
        source: Source::Artifact(&binary.path),
        sha256: &binary.sha256,
        bytes: binary.bytes,
    });
    install_programs(programs, install_dir, retained, record_directory)
}

/// 只把回滚记录保留的三个角色程序装回安装目录，auction-cli 保持不变；程序经记录目录的固定
/// 描述符读取，安装时再按记录摘要复核。不变量与 [`install_artifact`] 相同，只是换回来源是
/// `reverse`（rollback 停机后刚保留的回退前角色程序）；回退不执行迁移，换回发生在启动服务之前，
/// 换回失败时不重试。
pub(crate) fn install_retained(
    record: &RollbackRecord,
    record_directory: &RecordDirectory,
    install_dir: &Path,
    reverse: &RollbackRecord,
    reverse_directory: &RecordDirectory,
) -> Result<(), InstallArtifactFailure> {
    let mut programs = Vec::with_capacity(ROLE_BINARIES.len());
    for name in ROLE_BINARIES {
        let Some(binary) = record.binary(name) else {
            // 记录已在读取时校验过三个角色；缺项说明记录在内存中被改动，什么都还没有替换。
            return Err(InstallArtifactFailure {
                error: anyhow::anyhow!("回滚记录缺少 {name}"),
                restored: true,
            });
        };
        programs.push(Program {
            name,
            source: Source::Retained(record_directory),
            sha256: &binary.sha256,
            bytes: binary.bytes,
        });
    }
    install_programs(programs.into_iter(), install_dir, reverse, reverse_directory)
}

/// 待安装程序的来源：制品中的文件按路径以 O_NOFOLLOW 打开（内容由制品摘要证明）；回滚记录中的
/// 保留程序只经记录目录的固定描述符打开。
enum Source<'a> {
    Artifact(&'a Path),
    Retained(&'a RecordDirectory),
}

impl Source<'_> {
    fn open(&self, name: &str) -> anyhow::Result<File> {
        match self {
            Self::Artifact(path) => OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .with_context(|| format!("读取待安装程序失败：{}", path.display())),
            Self::Retained(directory) => directory.open_program(name).with_context(|| {
                format!("读取待安装程序失败：{}", directory.path().join("bin").join(name).display())
            }),
        }
    }
}

/// 待安装的单个程序与它的期望摘要。
struct Program<'a> {
    name: &'a str,
    source: Source<'a>,
    sha256: &'a str,
    bytes: u64,
}

/// 逐个替换 `programs`；任一程序失败时把本次已替换的程序（含 rename 之后失败的那一个）交给
/// 换回，并复核换回记录覆盖的全部程序。
fn install_programs<'a>(
    programs: impl Iterator<Item = Program<'a>>,
    install_dir: &Path,
    retained: &RollbackRecord,
    record_directory: &RecordDirectory,
) -> Result<(), InstallArtifactFailure> {
    let directory = match PinnedInstallDir::open(install_dir) {
        Ok(directory) => directory,
        // 还没有替换任何程序，无需换回；只有记录覆盖的程序摘要仍与记录一致时才声称原状。
        Err(error) => {
            let restored = retained.binaries.iter().all(|binary| {
                sha256_path(&install_dir.join(&binary.name))
                    .is_ok_and(|(sha256, bytes)| sha256 == binary.sha256 && bytes == binary.bytes)
            });
            return Err(InstallArtifactFailure {
                error: error.context("安装前复核安装目录失败，没有替换任何程序"),
                restored,
            });
        }
    };
    let swap = restore::Swap { retained, record_directory, directory: &directory };
    let mut replaced = Vec::with_capacity(super::artifact::REQUIRED_BINARIES.len());
    for program in programs {
        if let Err(failure) =
            install_one(&directory, &program.source, program.name, program.sha256, program.bytes)
        {
            if failure.replaced {
                replaced.push(program.name);
            }
            progress(format_args!(
                "安装 {} 失败，从回滚记录换回本次已替换的程序并复核安装目录",
                program.name
            ));
            return Err(swap.back(failure.error, &replaced));
        }
        replaced.push(program.name);
    }
    Ok(())
}

/// 单个程序的安装失败；`replaced` 表示失败发生在 rename 之后，目标可能已是新程序。
struct InstallFailure {
    error: anyhow::Error,
    replaced: bool,
}

/// 以 O_NOFOLLOW 读源程序，在固定的安装目录内写满临时文件并定为可信属主与 0755，复核摘要后
/// rename 覆盖目标，再重新打开目标复核属主、权限与摘要。
fn install_one(
    directory: &PinnedInstallDir,
    source: &Source<'_>,
    name: &str,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), InstallFailure> {
    let mut temporary = None;
    let mut replaced = false;
    let result = (|| -> anyhow::Result<()> {
        let mut input = source.open(name)?;
        let metadata = input.metadata().context("读取待安装程序元数据失败")?;
        if !metadata.is_file() || metadata.len() != expected_bytes {
            anyhow::bail!("待安装程序不是符合记录大小的普通文件：{name}");
        }
        let (staged, mut output) = directory.create_temporary(name)?;
        let staged: &str = temporary.insert(staged);
        copy_contents(&mut input, &mut output)?;
        directory.seal(&output)?;
        drop(output);
        let inspection = directory.inspect(staged)?;
        if inspection.sha256 != expected_sha256 || inspection.bytes != expected_bytes {
            anyhow::bail!("安装临时文件的摘要与期望不一致：{name}");
        }
        if !inspection.sealed {
            anyhow::bail!("安装临时文件的属主或权限不符：{name}");
        }
        directory.rename(staged, name).with_context(|| {
            format!("原子替换程序失败：{}", directory.path().join(name).display())
        })?;
        replaced = true;
        directory.sync()?;
        let installed = directory.inspect(name)?;
        if installed.sha256 != expected_sha256 || installed.bytes != expected_bytes {
            anyhow::bail!("安装后的程序摘要不一致：{name}");
        }
        if !installed.sealed {
            anyhow::bail!("安装后的程序属主或权限不符：{name}");
        }
        Ok(())
    })();
    result.map_err(|error| {
        // rename 成功后临时名已不存在；失败路径只回收本次创建的临时名。
        if !replaced && let Some(temporary) = &temporary {
            let _ = directory.remove(temporary);
        }
        InstallFailure { error, replaced }
    })
}

#[cfg(test)]
mod tests;
