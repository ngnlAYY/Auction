//! 页面字节来源：内嵌产物与启动时整体校验过的目录挂载共用的只读视图。

use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;

use axum::body::Bytes;
use rust_embed::{EmbeddedFile, RustEmbed};

/// 一个页面文件的只读视图：原文或预压缩实体，带媒体类型与内容摘要。
#[derive(Clone)]
pub(crate) struct AssetFile {
    pub(crate) data: Bytes,
    pub(crate) mimetype: String,
    /// 内容摘要，也是未混入策略标识时的 `ETag` 来源。
    pub(crate) sha256: [u8; 32],
    /// 只有内嵌文件携带构建时间；挂载文件没有可信的日期验证器。
    pub(crate) last_modified: Option<u64>,
}

impl From<EmbeddedFile> for AssetFile {
    fn from(file: EmbeddedFile) -> Self {
        let data = match file.data {
            Cow::Borrowed(bytes) => Bytes::from_static(bytes),
            Cow::Owned(bytes) => Bytes::from(bytes),
        };
        Self {
            data,
            mimetype: file.metadata.mimetype().to_owned(),
            sha256: file.metadata.sha256_hash(),
            last_modified: file.metadata.last_modified(),
        }
    }
}

/// 某个 build 的页面字节来源；内嵌与目录挂载各一个实现。`get` 只接受清单内的相对路径。
pub(crate) trait AssetSource: Send + Sync + 'static {
    fn get(&self, relative_path: &str) -> Option<AssetFile>;

    /// 惰性列出全部相对路径；站点图标按稳定别名查找时使用。
    fn paths(&self) -> Box<dyn Iterator<Item = Cow<'_, str>> + '_>;

    /// 带内容指纹的站点图标路径：`assets/favicon-<指纹>.svg`。
    ///
    /// 挂载来源的路径按 `HashMap` 顺序枚举；多个候选时取字典序最小者，
    /// 保证同一份挂载在每次启动都返回同一表示（ETag 与正文不随迭代顺序变化）。
    fn favicon_path(&self) -> Option<String> {
        self.paths()
            .filter(|path| path.starts_with("assets/favicon-") && path.ends_with(".svg"))
            .min()
            .map(Cow::into_owned)
    }
}

/// 编译进二进制（或开发态从磁盘读取）的 `RustEmbed` 产物。
pub(crate) struct EmbeddedSource<A>(PhantomData<fn() -> A>);

impl<A> EmbeddedSource<A> {
    pub(crate) fn new() -> Self {
        Self(PhantomData)
    }
}

impl<A: RustEmbed + 'static> AssetSource for EmbeddedSource<A> {
    fn get(&self, relative_path: &str) -> Option<AssetFile> {
        A::get(relative_path).map(AssetFile::from)
    }

    fn paths(&self) -> Box<dyn Iterator<Item = Cow<'_, str>> + '_> {
        Box::new(A::iter().map(narrow_lifetime))
    }
}

/// trait 对象里迭代器的 Item 不可协变，逐项把 `'static` 收窄到借用 `self` 的生命周期。
fn narrow_lifetime<'a>(path: Cow<'static, str>) -> Cow<'a, str> {
    path
}

/// 启动时从挂载目录整体读入并逐文件核对过的页面；请求期只是内存查表，不再触碰文件系统。
pub(crate) struct MountedSource {
    files: HashMap<String, AssetFile>,
}

impl MountedSource {
    pub(super) fn new(files: HashMap<String, AssetFile>) -> Self {
        Self { files }
    }
}

impl AssetSource for MountedSource {
    fn get(&self, relative_path: &str) -> Option<AssetFile> {
        self.files.get(relative_path).cloned()
    }

    fn paths(&self) -> Box<dyn Iterator<Item = Cow<'_, str>> + '_> {
        Box::new(self.files.keys().map(|path| Cow::Borrowed(path.as_str())))
    }
}
