//! 备份范围明文与配对暂存文件的安全生命周期。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::support::files;

/// 发布前成对暂存的密文与 sidecar；未完成配对时回滚两个路径。
pub(super) struct ScopePairTemps {
    bundle: PathBuf,
    sidecar: PathBuf,
    preserve: bool,
}

impl ScopePairTemps {
    pub(super) fn new(bundle: PathBuf, sidecar: PathBuf) -> Self {
        Self { bundle, sidecar, preserve: false }
    }

    pub(super) fn preserve(&mut self) {
        self.preserve = true;
    }

    pub(super) fn cleanup(&mut self) -> anyhow::Result<()> {
        self.preserve = false;
        remove_if_present(&self.sidecar)?;
        remove_if_present(&self.bundle)?;
        Ok(())
    }
}

impl Drop for ScopePairTemps {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = remove_if_present(&self.sidecar);
            let _ = remove_if_present(&self.bundle);
        }
    }
}

pub(super) fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("清理范围暂存文件失败：{}", path.display()))
        }
    }
}

/// 明文暂存在任何退出路径上都被删除；封存成功后立即主动删除。
pub(super) struct PlaintextGuard {
    path: PathBuf,
    armed: bool,
}

impl PlaintextGuard {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(super) fn remove(&mut self) -> anyhow::Result<()> {
        if !self.armed {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("删除范围明文暂存失败：{}", self.path.display()));
            }
        }
        self.armed = false;
        files::sync_parent(&self.path)
    }
}

impl Drop for PlaintextGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}
