//! 灾备目录链的属主和替换权限检查；可信 UID 仅当前进程与 root。

use std::path::Path;

pub(super) fn verify_parent_chain(parent: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        // 父级已验证为绝对 canonical 路径；逐层排除非可信 UID 改写目录或替换祖先的能力。
        // SAFETY: geteuid 不接收指针，仅返回当前进程有效 UID。
        let uid = unsafe { libc::geteuid() };
        for directory in parent.ancestors() {
            let metadata = std::fs::symlink_metadata(directory)?;
            if !metadata.is_dir() || (metadata.uid() != uid && metadata.uid() != 0) {
                anyhow::bail!("recovery artifact directory must be owned by current UID or root");
            }
            let writable_by_others = metadata.mode() & 0o022 != 0;
            // 直接父目录始终禁止共享写入；祖先只允许 root 所有的 sticky 目录，
            // 且下一层已验证为可信 owner，其他 UID 因而不能替换其目录项。
            let protected_ancestor =
                directory != parent && metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
            if writable_by_others && !protected_ancestor {
                anyhow::bail!("recovery artifact directory chain allows untrusted replacement");
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        anyhow::bail!("recovery artifact directory ownership requires Unix")
    }
}
