//! 路径信任的纯判定：按目录项的类型、属主与权限位判断它能否被他人改名、替换或改写。判定与真实
//! 文件解耦，非 root 的测试也能用合成元数据覆盖 root 口径。私有目录、部署状态目录与安装目录的
//! 祖先共用这一份规则。

use std::{fs::Metadata, os::unix::fs::MetadataExt};

/// 目录项类型；符号链接与特殊文件都归入 `Other`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Directory,
    File,
    Other,
}

/// 判定所需的元数据事实。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts {
    pub(crate) kind: Kind,
    pub(crate) uid: u32,
    pub(crate) mode: u32,
}

impl Facts {
    /// 取自 `symlink_metadata` 或已打开描述符的元数据；符号链接记为 `Other`。
    pub(crate) fn of(metadata: &Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            Kind::Other
        } else if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        Self { kind, uid: metadata.uid(), mode: metadata.mode() }
    }
}

/// 祖先目录：真实目录、归 root 或当前账号、禁止 group/other 写入。root 拥有的 sticky 目录除外：
/// 他人不能改名或删除其中不属于自己的目录项。
pub(crate) fn ancestor_problem(facts: Facts, euid: u32) -> Option<&'static str> {
    if facts.kind != Kind::Directory {
        return Some("不是真实目录");
    }
    if facts.uid != 0 && facts.uid != euid {
        return Some("属主不是 root 或当前账号");
    }
    let sticky_root = facts.uid == 0 && facts.mode & libc::S_ISVTX != 0;
    if facts.mode & 0o022 != 0 && !sticky_root {
        return Some("允许 group/other 写入");
    }
    None
}

/// 从根开始逐级的事实中第一个不可信的层级与原因；任一层级可被他人改名或替换，其下的整棵树都
/// 不可信。
pub(crate) fn first_untrusted(chain: &[Facts], euid: u32) -> Option<(usize, &'static str)> {
    chain
        .iter()
        .enumerate()
        .find_map(|(index, facts)| ancestor_problem(*facts, euid).map(|problem| (index, problem)))
}

#[cfg(test)]
mod tests {
    use super::{Facts, Kind, ancestor_problem, first_untrusted};

    const SERVICE_UID: u32 = 990;

    fn directory(uid: u32, mode: u32) -> Facts {
        Facts { kind: Kind::Directory, uid, mode: 0o40_000 | mode }
    }

    /// root 运行时（部署状态目录的标准口径）只信任 root 的祖先：标准布局 `/var/lib/auction` 为
    /// root:root 0755 时通过；它归服务账号所有（旧布局）或允许 group/other 写入时，在该层级拒绝。
    /// root 拥有的 sticky 目录（如 /tmp）可以，服务账号拥有的 sticky 目录不行。
    #[test]
    fn root_trusts_only_root_owned_ancestors() {
        let standard = [
            directory(0, 0o755),
            directory(0, 0o755),
            directory(0, 0o755),
            directory(0, 0o755),
            directory(0, 0o700),
        ];
        assert_eq!(first_untrusted(&standard, 0), None);

        let mut legacy = standard;
        legacy[3] = directory(SERVICE_UID, 0o750);
        assert_eq!(first_untrusted(&legacy, 0), Some((3, "属主不是 root 或当前账号")));
        legacy[3] = directory(0, 0o775);
        assert_eq!(first_untrusted(&legacy, 0), Some((3, "允许 group/other 写入")));
        legacy[3] = Facts { kind: Kind::Other, uid: 0, mode: 0o120_777 };
        assert_eq!(first_untrusted(&legacy, 0), Some((3, "不是真实目录")));

        assert_eq!(ancestor_problem(directory(0, 0o1777), 0), None, "root 的 sticky 目录");
        assert!(ancestor_problem(directory(SERVICE_UID, 0o1777), 0).is_some());
    }

    /// 非 root 的隔离环境：祖先可以归 root 或当前账号，第二个账号拥有的祖先一律拒绝。
    #[test]
    fn non_root_trusts_itself_and_root_but_not_a_second_account() {
        let euid = 1000;
        let chain = [directory(0, 0o755), directory(euid, 0o700), directory(euid, 0o700)];
        assert_eq!(first_untrusted(&chain, euid), None);
        let mut shared = chain;
        shared[1] = directory(SERVICE_UID, 0o755);
        assert_eq!(first_untrusted(&shared, euid), Some((1, "属主不是 root 或当前账号")));
    }
}
