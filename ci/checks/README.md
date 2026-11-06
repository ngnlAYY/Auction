# 静态门禁

[ci/README.md](../README.md) · [docs/development.md](../../docs/development.md)

[ci/checks/check.sh](check.sh) 是本地与 CI 的共同入口。`all` 包含仓库契约、目录布局、配置示例、systemd 模板、翻译键、装备诊断闭集、源码计数棘轮、Markdown 链接、迁移兼容登记与已发布迁移不可变检查；完整清单见 [ci/README.md#静态门禁cicheckschecksh](../README.md#静态门禁cicheckschecksh)。`just verify` 先运行 `all`，再执行格式、编译、feature、依赖版本与 Clippy/WASM 检查，不运行行为测试。

## 迁移兼容登记

```bash
bash ci/checks/check.sh migration-compatibility
```

[ci/checks/migration-compatibility.pl](migration-compatibility.pl) 读取迁移文件，对编号大于 074 的文件执行词法与正则约束；001–074 不要求登记且原文件不能改写。文件名须为六位编号与 snake_case 描述，新编号自 075 连续、不得重复。

词法扫描区分行注释、嵌套块注释、单引号/转义字符串、双引号标识符及带标签的 dollar quote；只在顶层分号切分 SQL，函数与 DO 内的 INSERT 也计数但不能充当合法登记，EXECUTE 字符串中的登记表引用拒绝。普通字符串与非过程 dollar quote 中的示例不计为登记。每个新文件须恰好一次 `INSERT INTO public.schema_migration_compatibility (version, level) VALUES (十进制本版本, 'compatible' 或 'breaking');`，必须是最后一条无条件顶层 SQL。允许关键字大小写及空白变化，等级字面量保持小写；禁止 upsert、INSERT SELECT、多行 VALUES、过程内登记或用注释/字符串冒充登记。

禁止 `-- no-transaction`，顶层 BEGIN/START TRANSACTION、COMMIT/END、ROLLBACK/ABORT、PREPARE/SET TRANSACTION，保持 SQLx 对迁移与登记的事务边界。失败输出文件与原因并退出 1。该门禁不执行 SQL、不连接数据库、不证明 compatible 的业务承诺；分类审阅规则由[migrations/postgres/README.md#兼容分级与登记](../../migrations/postgres/README.md#兼容分级与登记)拥有，跨版本验证仍由 W7.8 后续工作负责。

## 已发布迁移不可变

```bash
bash ci/checks/check.sh published-migrations
```

[ci/checks/published-migrations.sh](published-migrations.sh) 把每个 `v<数字>` 发布 tag 视为已发布版本，读取该 tag 的 `migrations/postgres/*.up.sql`，要求当前工作树中同一路径仍然是普通文件、且字节内容与 tag 中的 Git blob 完全一致。修改、重命名或删除任一已发布迁移都会失败；追加新编号文件不受影响。检查只比较 Git 对象与工作树，不执行 SQL、不联网，也不修改 tag。

为实现失败关闭，检查拒绝浅克隆：`git rev-parse --is-shallow-repository` 必须为 `false`，CI 的 `docs` 作业因此使用 `fetch-depth: 0`。本地缺少 tag 时先执行 `git fetch --tags`；没有发布 tag 时输出“仅有未发布的迁移”并通过。发布工作流的 `guard` 作业在构建前独立运行这一检查。

## unwrap/expect 门禁

根 [Cargo.toml](../../Cargo.toml) 的 workspace Clippy 将 `unwrap_used`、`expect_used` 设为 `deny`；所有成员继承统一策略。`lint` 检查 native 全工作区、全部 target 与 feature，`wasm` 分别检查 Public/Admin 浏览器及共享依赖，覆盖只在 `wasm32` 编译的分支。构建脚本随所属成员检查；Clippy 会跳过部分宏展开，因此新增或修改本仓库宏时仍须核对定义中的 `unwrap`/`expect`，不能只依靠调用点的 Clippy 结果。

根 [clippy.toml](../../clippy.toml) 使用 `allow-unwrap-in-tests = true`、`allow-expect-in-tests = true` 豁免测试上下文（包括 `#[test]` 与 `#[cfg(test)]` 模块），不在整个测试构建中放宽生产函数。生产代码须显式传播错误或处理 `None`；不可恢复的启动不变量也应给出可行动的错误。必要例外只允许最小范围的 `#[expect(clippy::unwrap_used, reason = "中文原因")]` 或对应 `expect_used`，不得 crate 级关闭。

回退时移除上述两个 workspace lint 与测试配置即可；既有安全错误处理保持不变。

## 源码计数棘轮

```bash
bash ci/checks/check.sh ratchet         # 与基线比较
bash ci/checks/check.sh ratchet-counts  # 仅输出当前计数，不修改基线
```

[ci/checks/source-ratchet.pl](source-ratchet.pl) 通过 `rg` 枚举 `apps/*/src/**/*.rs` 与 `crates/*/src/**/*.rs`（包括 `src` 根文件），遵循 Git 忽略规则，排除任意 `tests/` 与 `target/` 目录。`tests.rs`、其他测试文件名和内联 `#[cfg(test)]` 模块仍计入；不解析条件编译，因此未启用的 feature/平台代码也计入。注释（含嵌套块注释）、普通/原始/字节/字符字面量不计入。

计数按出现次数而非行数，多行写法同样计入。这是文本模式棘轮，不推断变量是否实际使用，也不展开宏或解析完整 Rust 语法。

| 基线模式 | 口径 |
| --- | --- |
| `all:#[path]` | 全部上述源码中直接的 `#[path = …]` / `#![path = …]` 属性；不含 `cfg_attr` 内嵌属性 |
| `all:use_super::*` | 全部上述源码中的直接 `use super::*`；不含带花括号的导入 |
| `层:Err(_)` | 单一参数为 `_` 或下划线开头标识符的 `Err(…)`；包括 `_error`、`ref _error`、`mut _error` 与 `ref mut _error`；无论位于匹配、表达式还是宏参数中均计入 |
| `层:map_err(\|_\|)` | `map_err` 的单参数闭包以 `_` 或下划线开头标识符接收错误；包括 `move`、`mut _error`；不含显式参数类型或其他模式 |

错误模式分别统计六层：`jobs`、`application`、`http`、`storage`、`integrations` 对应同名 `crates/auction-*/src/`，`apps` 合并全部 `apps/*/src/`。注释和字面量被空白替代后再匹配，允许 token 之间存在任意空白。

基线在 [ci/checks/source-ratchet.baseline](source-ratchet.baseline)，每行一个 `层:模式 数量`，共 14 项。缺项、重复项、未知项与无效数量均失败关闭。当前计数高于基线时打印全部超出项并退出 1；低于基线时提示更新并继续通过。基线只能下调，须随清理改动一起审阅；不得为放行新增模式而上调。

更新时先生成候选，核对差异仅下降，再用候选替换基线并运行 `ratchet` 与 `just verify`：

```bash
ratchet_snapshot=$(mktemp)
bash ci/checks/check.sh ratchet-counts > "$ratchet_snapshot"
diff -u ci/checks/source-ratchet.baseline "$ratchet_snapshot"
# 人工核对只下降后再执行：
cp "$ratchet_snapshot" ci/checks/source-ratchet.baseline
rm "$ratchet_snapshot"
bash ci/checks/check.sh ratchet
```

## Markdown 相对链接

```bash
bash ci/checks/check.sh markdown-links
```

[ci/checks/markdown-links.pl](markdown-links.pl) 检查仓库全部未被忽略的 `.md` 文件（扩展名大小写均可，包含隐藏目录中的文档），排除 `.git/` 与任意 `target/`。支持行内链接、图片及已定义的完整/折叠/快捷引用链接；目标按所在文档目录解析，检查文件或目录是否存在。查询串不参与路径检查，路径和锚点的百分号编码按 UTF-8 解码。围栏/缩进代码块、行内代码及 HTML 注释中的示例不作为链接。

Markdown 同页与跨页锚点按 GitHub 标题规则生成：ATX（`#`）或 Setext（下划线）标题先去掉链接目标、HTML 与行内格式标记，保留可见文本，再转小写、去标点和符号、空白转 `-`；保留中文、下划线与连字符。同名标题追加 `-1`、`-2`，遇到已有同名锚点继续递增避让。支持显式 HTML `a` / `h1`–`h6` 的 `id` / `name`。目录链接带锚点时检查其中的 `README.md`；非 Markdown 文件仅接受存在的 GitHub `#L行号` / `#L起始-L结束` 锚点。

不访问网络：`http(s)://`、`mailto:` 及其他 URI scheme、`//` 协议相对地址与 `/` 站点绝对路径均跳过。失败输出包含来源文件、行号、原目标及缺失路径或锚点；不维护断链忽略清单。移动或重命名文档时须同步修复链接，正确目标不明确时交由维护者决定。

## 每周依赖审计

[.github/workflows/ci.yml](../../.github/workflows/ci.yml) 每周一 **04:17 UTC** 调度现有 `security-audit` 作业，复用受控 cargo-audit、工具链与缓存。调度事件只执行 `dependency-audit`，读取默认分支 `Cargo.lock` 并获取最新 RustSec 公告；漏洞及警告令作业失败，不修改依赖、不自动发布。push / PR / 手动事件继续保留原有门禁与未使用依赖检查。最近一个发布 tag 的周期审计属于 W13.5，尚未接入；发布工作流本身不替代周期审计。

回退时从 `all` 移除对应检查，并移除其脚本/基线；定时审计可移除 `schedule` 及其专用条件。均不改变应用运行时、路由、JSON、事件、配置或数据库契约。
