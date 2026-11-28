//! 入口指针 `ENTRY.v1` 的读写、切换与只读状态。
//!
//! 不变量：指针是一项不可分割的版本记录（默认 build 与独立 build 一起换），经同目录临时文件、
//! `fsync` 与 `rename` 原子替换；换之前两边 build 目录都重新逐文件核对，独立入口始终是 Solid，
//! 回切旧壳只改默认入口、永不改独立入口。服务只在启动时读取指针，切换后须重启 Public/Admin。

use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;
use serde_json::{Value, json};
use uuid::Uuid;

use super::layout::{ENTRY_FILE, EntryPointer, MAX_ENTRY_BYTES, Stack, Surface, validate_build_id};
use super::verify::{BuildFacts, reject_conflicts, require_real_directory, verify_build_dir};
use crate::support::files;

/// 切换目标：`old` 是物化的旧 Dioxus 壳，`new` 是 Solid 制品。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum SwitchTarget {
    Old,
    New,
}

impl SwitchTarget {
    const fn stack(self) -> Stack {
        match self {
            Self::Old => Stack::Dioxus,
            Self::New => Stack::Solid,
        }
    }
}

/// 读取指针；文件不存在返回 `None`，其余异常（符号链接、过大、格式不符、surface 不符）失败。
pub(super) fn read_pointer(
    surface_root: &Path,
    surface: Surface,
) -> anyhow::Result<Option<EntryPointer>> {
    let path = surface_root.join(ENTRY_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取入口指针失败：{}", path.display()));
        }
    }
    let bytes = files::read_bounded(&path, MAX_ENTRY_BYTES)?;
    let text = String::from_utf8(bytes).context("入口指针不是 UTF-8")?;
    let pointer = EntryPointer::parse(&text)?;
    if pointer.surface != surface {
        anyhow::bail!("入口指针的 surface 与所在目录不符");
    }
    Ok(Some(pointer))
}

/// 同目录临时文件 + fsync + rename + 目录 fsync；失败时临时文件被删除、原指针不变。
pub(super) fn write_pointer(surface_root: &Path, pointer: &EntryPointer) -> anyhow::Result<()> {
    publish_pointer(surface_root, pointer, true).map(|_| ())
}

/// 发布入口指针；`replace=false` 时以 `RENAME_NOREPLACE` 只在目标不存在时成功，返回是否发布。
/// 首次安装与 `switch` 共用同一临时文件流程，区别只在最终改名是否允许覆盖。
fn publish_pointer(
    surface_root: &Path,
    pointer: &EntryPointer,
    replace: bool,
) -> anyhow::Result<bool> {
    let temporary = surface_root.join(format!(".{ENTRY_FILE}.tmp-{}", Uuid::new_v4()));
    let result = (|| -> anyhow::Result<bool> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)
            .context("创建入口指针临时文件失败")?;
        file.write_all(pointer.render().as_bytes()).context("写入入口指针失败")?;
        file.sync_all().context("同步入口指针失败")?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644))
            .context("设置入口指针权限失败")?;
        let target = surface_root.join(ENTRY_FILE);
        if replace {
            fs::rename(&temporary, target).context("替换入口指针失败")?;
            return Ok(true);
        }
        match rename_no_replace(&temporary, &target) {
            Ok(()) => Ok(true),
            // 另一个写入者先建立了指针：目标与目录项都未变，如实报告未发布。
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error).context("发布入口指针失败"),
        }
    })();
    let published = match result {
        Ok(published) => published,
        Err(error) => {
            return match fs::remove_file(&temporary) {
                Ok(()) => Err(error),
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(error),
                Err(cleanup) => Err(error.context(format!("且清理临时文件失败：{cleanup}"))),
            };
        }
    };
    if !published {
        return match fs::remove_file(&temporary) {
            Ok(()) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("清理未发布的入口指针临时文件失败"),
        };
    }
    files::sync_directory(surface_root, "surface 目录")?;
    Ok(true)
}

/// 指针指向的默认 build 与独立 build 构成的记录；`independent` 必须是 Solid。
fn pointer_for(
    surface: Surface,
    default: &BuildFacts,
    independent: &BuildFacts,
) -> anyhow::Result<EntryPointer> {
    if independent.manifest.stack != Stack::Solid {
        anyhow::bail!("独立入口必须是 Solid build：{}", independent.manifest.build_id);
    }
    if default.manifest.build_id != independent.manifest.build_id {
        reject_conflicts(&default.manifest, &independent.manifest)?;
    }
    Ok(EntryPointer {
        surface,
        build_id: default.manifest.build_id.clone(),
        stack: default.manifest.stack,
        entry_sha256: default.index_sha256.clone(),
        payload_sha256: default.manifest.payload_sha256.clone(),
        independent_build_id: independent.manifest.build_id.clone(),
        independent_payload_sha256: independent.manifest.payload_sha256.clone(),
    })
}

/// 首次安装新 build 时建立指针：默认与独立入口都是它。指针已存在时不改动，返回 `false`；
/// 无覆盖发布保证并发 `install` 只有一个能建立首次指针，后到者不会替换已提交的入口。
pub(super) fn init_pointer(
    surface_root: &Path,
    surface: Surface,
    build_id: &str,
) -> anyhow::Result<bool> {
    if read_pointer(surface_root, surface)?.is_some() {
        return Ok(false);
    }
    let facts = verify_build_dir(surface_root, surface, build_id)?;
    publish_pointer(surface_root, &pointer_for(surface, &facts, &facts)?, false)
}

/// 规划把入口切到 `build_id`：只核对、不写任何文件，返回将要换入的指针。`to` 只是对目标栈的断言
/// （`old` 必须是 dioxus，`new` 必须是 solid），防止把构建号敲错后换到另一种栈。
pub(super) fn plan_switch(
    web_root: &Path,
    surface: Surface,
    to: SwitchTarget,
    build_id: &str,
) -> anyhow::Result<EntryPointer> {
    validate_build_id(build_id)?;
    require_real_directory(web_root)?;
    let surface_root = web_root.join(surface.as_str());
    require_real_directory(&surface_root)?;
    let current = read_pointer(&surface_root, surface)?
        .context("入口指针不存在：先用 web install 安装新版 build")?;

    let target = verify_build_dir(&surface_root, surface, build_id)?;
    if target.manifest.stack != to.stack() {
        anyhow::bail!(
            "目标 build 的栈是 {}，与 --to 不符：{build_id}",
            target.manifest.stack.as_str()
        );
    }
    let next = match to {
        // 新版 Solid：默认与独立入口一起换到它。
        SwitchTarget::New => pointer_for(surface, &target, &target)?,
        // 回切旧壳：只改默认入口，独立入口保持当前的 Solid build，换之前同样重新核对。
        SwitchTarget::Old => {
            let independent =
                verify_build_dir(&surface_root, surface, &current.independent_build_id)?;
            pointer_for(surface, &target, &independent)?
        }
    };
    Ok(next)
}

/// 只读状态：每个 surface 的指针与各 build 的核对结果；不写任何文件。
pub(super) fn status(web_root: &Path) -> anyhow::Result<Value> {
    require_real_directory(web_root)?;
    let mut surfaces = serde_json::Map::new();
    for surface in [Surface::Public, Surface::Admin] {
        let surface_root = web_root.join(surface.as_str());
        if fs::symlink_metadata(&surface_root).is_err() {
            continue;
        }
        require_real_directory(&surface_root)?;
        let pointer = match read_pointer(&surface_root, surface) {
            Ok(Some(pointer)) => json!({
                "build_id": pointer.build_id,
                "stack": pointer.stack.as_str(),
                "entry_sha256": pointer.entry_sha256,
                "payload_sha256": pointer.payload_sha256,
                "independent_build_id": pointer.independent_build_id,
                "independent_payload_sha256": pointer.independent_payload_sha256,
            }),
            Ok(None) => Value::Null,
            Err(error) => json!({ "error": format!("{error:#}") }),
        };
        surfaces.insert(
            surface.as_str().to_owned(),
            json!({ "pointer": pointer, "builds": builds(&surface_root, surface)? }),
        );
    }
    Ok(json!({ "web_root": web_root.display().to_string(), "surfaces": surfaces }))
}

/// 目录里全部 build 的名字（不含以点开头的暂存目录与临时文件），已排序。
fn build_names(surface_root: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(surface_root).context("读取 surface 目录失败")? {
        let entry = entry.context("读取目录项失败")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().context("读取文件类型失败")?.is_dir() && !name.starts_with('.')
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// 逐个核对 surface 下的全部 build；任何一个核对失败都整体失败。
pub(super) fn builds_of(surface_root: &Path, surface: Surface) -> anyhow::Result<Vec<BuildFacts>> {
    build_names(surface_root)?
        .iter()
        .map(|name| verify_build_dir(surface_root, surface, name))
        .collect()
}

fn builds(surface_root: &Path, surface: Surface) -> anyhow::Result<Vec<Value>> {
    Ok(build_names(surface_root)?
        .into_iter()
        .map(|name| match verify_build_dir(surface_root, surface, &name) {
            Ok(facts) => json!({
                "build_id": name,
                "stack": facts.manifest.stack.as_str(),
                "payload_sha256": facts.manifest.payload_sha256,
                "files": facts.manifest.files.len(),
                "verified": true,
            }),
            Err(error) => {
                json!({ "build_id": name, "verified": false, "error": format!("{error:#}") })
            }
        })
        .collect())
}
