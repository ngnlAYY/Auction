# CI 与发布

[docs/README.md](../docs/README.md) · [docs/development.md](../docs/development.md) · [ci/tools/README.md](tools/README.md) · [ops/README.md](../ops/README.md)

| 目录 | 内容 |
| --- | --- |
| [ci/checks](checks) | [ci/checks/README.md](checks/README.md)：[ci/checks/check.sh](checks/check.sh)（策略、格式、编译、lint、审计）与对应脚本/基线 |
| [ci/release](release) | 发布入口 [ci/release/release.sh](release/release.sh)、Web payload 构建 [ci/release/web-bundle.sh](release/web-bundle.sh)、共用身份函数 [ci/release/build-identity.sh](release/build-identity.sh)、部署制品来源校验 [ci/release/verify-deployment-package.sh](release/verify-deployment-package.sh)、体积预算 [ci/release/web-size-budget.v1](release/web-size-budget.v1) |
| [ci/tools](tools) | 工具清单与安装方式（mise）、rustup 基线脚本与本地 Blaze 试验入口 |

发布工具面向 Linux x86_64；装包还需 Perl（重定位文档链接）。CI 的 Cargo 编译、Clippy 与依赖图解析命令使用 `--locked`。格式检查和独立审计工具使用各自的参数。

## 静态门禁：`ci/checks/check.sh`

```bash
bash ci/checks/check.sh <检查项>...
```

| 检查项 | 内容 |
| --- | --- |
| `repository` | `AGENTS.md`、`.gitignore`、`.editorconfig`、`README.md`、`Cargo.lock`、`mise.toml`、`mise.lock` 必须纳入版本控制且未被忽略；任何已跟踪文件都不得命中忽略规则（`git ls-files -ci --exclude-standard` 为空，防止强制添加的本地配置与被 rg 类门禁静默跳过的文件） |
| `rust-version` | `Cargo.toml` 的 `workspace.package.rust-version`（MSRV）必须等于 `rust-toolchain.toml` 的 `channel`，每个工作区包都以 `rust-version.workspace = true` 继承；项目不为更旧的 Rust 保留兼容 |
| `public-redaction` | 仓库公开，未被 `.gitignore` 忽略的文件（`Cargo.lock` 除外）不得包含本机用户目录（`/home/<用户>/`、`/Users/<用户>/`）、192.168/16 内网地址或本机测试库名（`auction-test-` 紧跟 `public`、`admin`、`worker`、`{` 或 `*`）；测试夹具、`docs/`、`ops/`、`ci/`、`.github/` 与 Markdown 中的 IPv4 只允许回环、0.0.0.0 与 RFC 5737 文档段（Caddyfile 的 Cloudflare 网段按文件豁免）；测试夹具不得残留浏览器用户脚本注入。示例与夹具改用系统路径、`example.com` 与 RFC 5737 文档地址，口径见[ci/checks/README.md#公开仓库脱敏](checks/README.md#公开仓库脱敏) |
| `layout` | `src/` 根只能有 `lib.rs`/`main.rs`；每个 crate 的 `src/` 必须有职责子目录；同目录不得有 ≥3 个同前缀源码文件；目录模块入口为 `mod.rs`，不与同名 `.rs` 并存；文件与目录名 snake_case，不用 common/helpers/shared/misc/utils 兜底命名；生产源码（截止于首个内联测试模块）不超过 600 行（`mod.rs` 500 行）、不用私有 glob 导入（含 `use super::{a::*, *}` 等花括号形式），子模块不经 `super::`/`crate::` 路径借用祖先模块的私有 `use` 导入（从各 crate 根按模块树检查），每个 src 源文件都必须能从 crate 根到达（不留孤儿文件）；全部源码不用 `use { … }` 总括分组。行数、glob 与祖先导入口径见[ci/checks/README.md#模块层级门禁](checks/README.md#模块层级门禁) |
| `import-groups` | 导入块按 std → 外部 crate → 本 crate 三组排列；模块开头整个导入区的私有 `use` 组序不回退，首个非 use/mod 条目之后不得再出现模块级私有 `use`，口径见[ci/checks/README.md#导入分组](checks/README.md#导入分组) |
| `module-docs` | `apps/*/src` 与 `crates/*/src` 的每个非测试 `.rs` 文件首行必须是 `//!` 模块说明（`tests/`、`tests.rs`、`*_tests.rs` 除外） |
| `hot-fields` | Admin 两套参数页的热字段字面量数组必须与应用层闭集 `WORKER_HOT_FIELD_PATHS` 逐项同序，见[热字段镜像](checks/README.md#热字段镜像) |
| `systemd-templates` | [ops/deploy/systemd](../ops/deploy/systemd) 的 Public/Admin unit 模板须以 `After=`/`Wants=` 引用 Worker（Public 另含 Admin），角色之间不得用 `Requires=` 传播停止与失败 |
| `config` | `ops/config/examples/**/*.toml` 每个赋值字段（含注释掉的示例字段）前必须紧邻含“可选”或“必填”的注释 |
| `i18n` | [crates/auction-web-public/i18n](../crates/auction-web-public/i18n) 与 [crates/auction-web-admin/i18n](../crates/auction-web-admin/i18n) 恰好包含 en、ja、ko、zh-CN、zh-TW 五个 `.ftl`；各语言键集合与 `en.ftl` 一致，同一个键在五种语言里引用的变量集合完全相同；键名不得含点号、不得重复。代码中的字面量键调用——Rust `t!("…")` 与 TypeScript `t("…")` / `t("…", {…})`（含 `web/shared`、`web/public/src` 与 `web/admin/src`）——必须指向已存在的键，传入的参数集合必须与译文的变量集合一致（少传、多传都报错）；TypeScript 只接受 `t(键)` 与 `t(键, { 普通属性/简写属性 })`，无法静态核对的形态按文件与行号报错。动态键沿用“键必须能在字符串字面量中可达”的规则。`en.ftl` 的每个键必须在同一 surface 的 Rust 或 Solid 源码中可达（两套实现共用译文，`crates/` 与 `web/` 任一处可达即可，含设置表单 `{id}-help` 拼接规则），规则见 [ci/checks/i18n-unused-keys.pl](checks/i18n-unused-keys.pl)。译文还必须满足 TypeScript 加载器的平铺子集：每条消息是单行的 `键 = 文本`，占位只有 `{ $变量 }` 与 `{"…"}`；续行、选择表达式、属性、术语、消息引用与函数调用按文件与行号报错，规则见 [ci/checks/i18n-flat-subset.pl](checks/i18n-flat-subset.pl)。三项源码检查先枚举一次并核对 `rg` 退出码，目录缺失或读取失败即失败。自测见 [ci/checks/i18n-test.sh](checks/i18n-test.sh) |
| `equipment-contract` | 核对装备解析诊断原因闭集、两套 Admin 实现中逐原因对应的文案键与五种语言译文，见[装备诊断闭集](checks/README.md#装备诊断闭集) |
| `ratchet` | 源码计数不得超过仓库基线；下降时提示更新，口径与更新方式见[ci/checks/README.md#源码计数棘轮](checks/README.md#源码计数棘轮) |
| `ratchet-counts` | 仅输出全部计数项的当前值，不修改基线；供人工审阅下调 |
| `audit-writes` | `crates/auction-storage/src` 生产源码中每条写入 `audit_records` 的 SQL 字面量，须在同一执行语句内以 `.await.record_audit_failure()` 接入审计失败分类，口径见[ci/checks/README.md#审计写入失败分类](checks/README.md#审计写入失败分类) |
| `work-item-ids` | 仓库内未被 `.gitignore` 忽略的文本文件（含 `.github/`、`docs/`、`migrations/` 的文档）不得引用已删除计划台账的工作项编号（`W<数字>.<数字>`）或决策编号（`决策 <数字>`、`D-W<数字>…`）；不扫描已发布迁移 SQL（`migrations/postgres/*.sql`）、`Cargo.lock`、`mise.lock` 与测试夹具（`tests/fixtures/`、`tests/assets-fixture/`）。只豁免迁移演练回执的协议固定值 `W11.4`。Rust 源码与 Markdown 文档（测试夹具与列出词表的本文件除外）另不得出现台账措辞“本切片”“按决策”；“本批”在源码中另有运行期批次含义，不列入 |
| `test-module-paths` | `apps/*/tests` 与 `crates/*/tests` 不得出现 `#[path]`：`src` 部分已由计数棘轮 `all:#[path]`（基线 0）覆盖，本项只兜底测试树；规则见[ci/checks/README.md#模块层级门禁](checks/README.md#模块层级门禁) |
| `port-coverage` | 只读报告 `crates/auction-storage` 中存储端口方法的测试调用覆盖与未覆盖清单；不并入 `all`/`mise run verify`，不使命令失败，见[ci/checks/README.md#存储端口覆盖率报告不并入-mise-run-verify](checks/README.md#存储端口覆盖率报告不并入-mise-run-verify) |
| `markdown-links` | 离线检查全部 Markdown 的相对链接目标、同页与跨页锚点，并拒绝紧跟在表格行之后、会被渲染进表格的文本行，规则见[ci/checks/README.md#markdown-相对链接](checks/README.md#markdown-相对链接) |
| `migration-compatibility` | 075 起每个迁移末尾恰好登记自身一次，闭集等级、版本及事务规范见[ci/checks/README.md#迁移兼容登记](checks/README.md#迁移兼容登记) |
| `published-migrations` | 每个发布 tag（`v<版本>`）中存在的 `migrations/postgres/*.up.sql` 必须在当前工作树逐字节不变；禁止修改、重命名或删除已发布文件，只允许追加新编号 |
| `web-build-inputs` | 离线核对 Dioxus 指纹清单与浏览器 crate 的工作区路径依赖闭包，见[ci/checks/README.md#web-输入依赖闭包](checks/README.md#web-输入依赖闭包) |
| `web-callback-props` | 检查三个 Rust Web crate 的组件参数和 `Props` 字段（含 `cfg_attr` 条件声明），拒绝可选的 `EventHandler` 和 `Callback`，见[ci/checks/README.md#web-回调属性](checks/README.md#web-回调属性) |
| `web` | 在 `web/` 下运行 `bun run typecheck`、`bun run lint` 与包边界检查（`web/tooling/check-boundaries.ts`，用 TypeScript 编译器解析语法树；说明符按闭集分类，退出码 1 为越界、2 为工具/配置/依赖/解析失败；别名只能在 `web/tsconfig.base.json` 登记，源码与目标符号链接失败关闭）；调用 Bun 前确认本地 `tsc` 与 `biome` 是普通可执行文件，`web/node_modules` 缺失或不完整时立即失败并提示 `mise run web-prepare`，门禁自己不安装依赖、不联网。不在 `all`/`rust` 组内，由 `mise run verify` 末尾显式运行，见[ci/checks/README.md#web-包边界](checks/README.md#web-包边界) |
| `all` | `repository rust-version public-redaction layout import-groups module-docs hot-fields config systemd-templates i18n equipment-contract ratchet audit-writes sql-clock test-module-paths work-item-ids markdown-links migration-compatibility published-migrations web-build-inputs web-callback-props`，重复墙钟调用检查见[ci/checks/README.md#sql-时钟](checks/README.md#sql-时钟) |
| `fmt` | `cargo fmt --all -- --check` |
| `check` | `cargo check --locked --workspace --all-targets` |
| `lint` | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`；再以 `--features web-embed` 单独 Clippy 两个 API 与合并服务，覆盖 `--all-features`（等于两端 Solid）看不到的 Dioxus 内嵌分支 |
| `dependency-versions` | `auction-server` 正常依赖图只能包含一个 `tungstenite` 版本；Axum 的 WebSocket 错误按版本 downcast，跨版本并存会静默丢失 1009 close 契约 |
| `wasm` | 以 `wasm32-unknown-unknown` 目标运行 `cargo clippy -D warnings`：Public（连同 `auction-web-shared`、`auction-contracts`）与 Admin 分开检查，各自保留独立 feature 图；`cfg(target_arch = "wasm32")` 代码只在此项被 lint。随后以发布页面所用的 `wasm-release` profile 对两端各运行一次 `cargo check`，并用 `build.warnings=deny` 拒绝本仓库成员的 rustc 告警：只在发布构型出现的告警在 dev 构型里看不到。这一步不链接，也不运行 `dx` 与 `wasm-opt` |
| `rustdoc` | `cargo doc` 全工作区（含私有项与全部 feature）以 `-D rustdoc::all` 构建：文档注释里的符号链接、HTML 标签与代码块属性必须可解析，失效链接即过期注释 |
| `rust` | `fmt check dependency-versions lint wasm rustdoc` |
| `dependency-audit` | [ci/checks/dependency-audit.sh](checks/dependency-audit.sh) 以 `cargo audit --deny warnings` 审计 `Cargo.lock`（需要 mise 安装的 cargo-audit，联网读取 RustSec）；例外登记在 [ci/checks/dependency-audit.ignore](checks/dependency-audit.ignore)，只收两类：已核实不进入编译图的公告，以及没有已知漏洞、本仓库不直接依赖、只经某个上游最新版本传递引入的“无人维护”类公告；每条写明原因、影响范围与解除条件，逐条以 `--ignore` 传入，定时审计复查 |
| `web-dependency-audit` | 在 `web/` 下运行 `bun audit`，以 `web/bun.lock` 为准查询 JS 依赖的已知漏洞；需要 `web/package.json`，不要求预装 `node_modules`。`bun` 由 mise 提供，联网读取 npm 公告源，离线或镜像不可达时失败关闭；不并入 `all` 与 `mise run verify`，由 CI 的 `security-audit` 作业执行，见[ci/checks/README.md#web-依赖审计不并入-mise-run-verify](checks/README.md#web-依赖审计不并入-mise-run-verify) |
| `release-dependency-audit` | 取最高版本的发布 tag（`v<semver>`，口径同发布守卫），把其中的 `Cargo.lock` 读到临时目录后以同样参数审计；没有发布 tag 时提示并通过。需要该 tag 的提交对象，见[ci/checks/README.md#每周依赖审计](checks/README.md#每周依赖审计) |
| `dependency-unused` | `cargo machete`（0.10.1）；编译期另有 Cargo 内置的 `unused_dependencies`（`[workspace.lints.cargo]`，`deny`），见[ci/tools/README.md](tools/README.md) |

`mise run verify` = `all fmt check dependency-versions lint wasm rustdoc web`。默认门禁不运行测试；Web 行为测试用 `mise run web-test` 单独运行。

## GitHub Actions

### 普通 CI：`.github/workflows/ci.yml`

| 作业 | 触发 | 内容 |
| --- | --- | --- |
| `docs` | 所有分支 push / PR / 手动 | 策略门禁与格式（`ci/checks/check.sh all fmt`）；完整历史与 tag 用于 `published-migrations`；同一提交已有 PR 时跳过重复的分支 push。tag 推送只运行发布工作流，不重复普通 CI |
| `verify` | push / PR / 手动，`docs` 通过后 | 经 mise 安装 bun 并执行 `mise run web-prepare` 后运行 `ci/checks/check.sh check dependency-versions lint wasm rustdoc web`（工具链 1.100.0） |
| `test` | push / PR / 手动，`docs` 通过后 | 经 mise 安装 bun 并执行 `mise run web-prepare`；安装 PostgreSQL 18 后依次运行 `mise run test`、`mise run web-test`、`mise run test-db`（PostgreSQL 18 集成测试）、`mise run test-db-compat`（迁移兼容声明验证）与 `mise run test-wasm`（使用运行器自带的 Firefox 验证浏览器收包接线）。这些测试均不并入默认 `mise run verify`。测试构建以 `CARGO_PROFILE_DEV_DEBUG=0` 关闭调试信息，缩小构建缓存 |
| `security-audit` | push / PR / 手动中 `docs` 通过后；每周一 04:17 UTC 独立调度 | `dependency-audit`（cargo-audit 由 mise 安装）与 `web-dependency-audit`（bun 由 mise 安装）相互独立执行；定时与手动运行另以完整历史执行 `release-dependency-audit`；非定时事件另执行 `dependency-unused`，见[ci/checks/README.md#每周依赖审计](checks/README.md#每周依赖审计) |
| `msrv` | push / PR / 手动，`docs` 通过后 | `ci/checks/check.sh rust-version`：MSRV 等于固定工具链，编译已由 `verify` 覆盖，这里只核对版本声明；作业名是 `main` 的必需检查 |
| `artifacts` | 仅手动（`workflow_dispatch`） | 调用可复用工作流 `.github/workflows/release-build.yml`，手动输入 `web_stack`（`dioxus` 缺省或 `solid`）：输入在 checkout 后先经 `ci/release/build-identity.sh` 校验，非法值、空串或大小写不符在任何工具安装与缓存恢复前失败；缓存键使用两端实际组合（如 `public-dioxus_admin-dioxus` 或 `public-solid_admin-solid`）。随后由 mise 安装发布工具，选择 `solid` 时另装 bun 并运行 `mise run web-prepare`，再以 `ci/release/release.sh release-check` 构建并校验完整发布包，上传 1 天候选包；在 tag 上手动选择 `solid` 会立即失败并提示在分支上触发候选构建。选择 `solid` 时候选 Actions artifact 名称带 `-solid`，缺省保持原名，候选包不可部署，默认分支推送不再运行 |

普通 CI 的 `test` 与版本发布的 `verify` 都在准备完 Web 依赖后运行 `mise run gate-self-test`。依赖与失败诊断见[门禁自测](checks/README.md#门禁自测)。

### 依赖升级窗口提醒：`.github/workflows/dependency-cadence.yml`

每周一 **05:07 UTC** 与手动触发时运行 [ci/checks/dependency-cadence.pl](checks/dependency-cadence.pl)，以 [ops/dependencies/upgrade-windows.json](../ops/dependencies/upgrade-windows.json) 为准核对季度窗口。日历结构错误、窗口逾期仍为 `scheduled` 或缺少下一次窗口时作业失败；窗口进入 14 天提前提醒期或正在进行时输出 warning，并把窗口表写入作业摘要。作业只读取仓库文件与 UTC 日期，不修改日历、不创建 issue、不升级依赖、不联网。节奏、冻结日历与门禁见[ops/dependencies.md](../ops/dependencies.md)。

### 版本发布：`.github/workflows/release.yml`

只由 `v<版本>` 形式的 tag 推送触发（tag 必须等于 `Cargo.toml` 的 workspace version）。流程：

1. `guard`：校验 tag 名与工作区版本一致；完整 checkout 后确认 tag 提交是 `origin/main` 的祖先；运行 `published-migrations`，任何已发布迁移被改写或删除都拒绝发布。
2. `verify`：启用 bun 并执行 `mise run web-prepare`，在 tag 上重跑 `ci/checks/check.sh all fmt check dependency-versions lint wasm rustdoc web`、`mise run test`、`mise run web-test`、PostgreSQL 18 `mise run test-db`、`mise run test-db-compat` 与 `mise run test-wasm`；测试入口与 PostgreSQL 18 准备方式均与 `ci.yml` 的 `test` 作业一致。
3. `build`：复用 `.github/workflows/release-build.yml`，与普通 CI 相同的发布构建、缓存与门禁，且不传 `web_stack`（缺省 Dioxus），产出候选包。
4. `publish`：复核候选包 SHA-256，用 `actions/attest-build-provenance` 生成绑定本仓库、本工作流、tag 与构建提交的 SLSA 来源证明，再用部署端同一脚本自检，最后 `gh release create --verify-tag` 创建 GitHub Release 并附带 `*.tar.gz`、`SHA256SUMS`、`DEPENDENCIES.md`。

Release 资产永久保留；同一 tag 的发行一经创建不会覆盖（重复运行在 `gh release create` 前失败）。发布构建的候选 Actions artifact 仍只保留 1 天，正式资产只存在于 GitHub Release。

缓存：Rust 编译缓存按作业与工具链隔离，PR 不写缓存；发布产物、`target/dx` 与 `target/web` 以源码身份指纹为键单独缓存，键再带实际技术栈（`dioxus` 或 `solid`），两种组合互不命中。mise 安装的工具在普通作业里按 `mise.toml` 与 `mise.lock` 的摘要缓存，每个作业用 `MISE_ENABLE_TOOLS` 只启用自己需要的工具；发布流程（`release.yml` 与 `release-build.yml`）不恢复 mise 缓存，下载的工具每次按锁文件校验和验证，`dx` 每次从源码编译。

发布构建开始前，使用与发布入口相同的 sccache（mise 安装的固定版本）和 daemon 地址执行 `sccache rustc -vV`。缓存服务启动失败（例如 GitHub 缓存存储返回 `503 ServerBusy`）时，步骤发出 warning 并清空 `RUSTC_WRAPPER`，由 rustc 直接完成后续构建；实际编译错误与发布门禁失败仍使作业失败。发布工具的解析在探测前执行，任一工具缺失直接终止。sccache 0.19.0 的 `SCCACHE_IGNORE_SERVER_IO_ERROR` 不覆盖启动失败，因此不能替代此探测。

## 发布：`ci/release/release.sh`

```bash
bash ci/release/release.sh release-check [--single-binary] [--allow-dirty]   # 完整构建 + 门禁 + 装包
bash ci/release/release.sh package [--single-binary] [--allow-dirty]         # 只装配已构建制品
bash ci/release/release.sh build-api | build-embedded | verify-artifacts
bash ci/release/release.sh build-single | verify-single
bash ci/release/release.sh verify-web-size | source-fingerprint
bash ci/release/release.sh web-feature | web-build-id | run-web public|admin
bash ci/release/verify-deployment-package.sh <*.tar.gz> --tag <v版本>        # 部署前来源校验
```

对应的 mise 任务：`mise run build-api`、`mise run build-release`、`mise run build-single`、`mise run verify-artifacts`、`mise run verify-single`、`mise run web-size`、`mise run release-check`、`mise run package`、`mise run verify-package`。

### 制品变体

| 变体 | feature | 程序 | 用途 |
| --- | --- | --- | --- |
| API-only | （无） | Public、Admin | 不发布；证明未启用 `web-embed` 时二进制不含任何页面（资源目录固定为空，页面路由 503） |
| embedded | 两端按实际技术栈取 `web-embed` 或 `web-solid` | Public、Admin、Worker、auction-cli | 分体部署发布包 |
| single | `auction-server/web-embed`，再按两端实际技术栈追加 `web-solid-public`/`web-solid-admin` | auction-server、auction-cli | 合并部署发布包（`-single` 后缀） |

feature、产物目录、工具、体积预算、构建回执与复用身份全部由 `ci/release/build-identity.sh` 的逐 surface 技术栈选择得出，cargo feature 由其中的 `web_cargo_features` 汇总。缺省与显式 `AUCTION_WEB_STACK=dioxus` 得到原来的 `auction-public-api/web-embed,auction-admin-api/web-embed` 与 `auction-server/web-embed`；`solid` 表示 Public 与 Admin 都用 Solid，因此 embedded 为 `auction-public-api/web-solid,auction-admin-api/web-solid`，single 为 `auction-server/web-embed,auction-server/web-solid-public,auction-server/web-solid-admin`。三种合法组合（API-only、Dioxus、Solid）都可用于本地构建与分支上的手动候选构建；正式发行仍只产出 Dioxus。

**正式发行（`v<版本>` tag 触发的 Release）仍然只产出 Rust 版页面。** 发布脚本以 CI 的显式触发上下文判定正式发行：`GITHUB_ACTIONS=true` 且 `GITHUB_REF_TYPE=tag`；此时任一端为 Solid 会在任何工具、临时目录、构建或装包步骤之前以“正式发行只产出 Rust 版页面”拒绝。本地构建与手动触发（`workflow_dispatch`，`GITHUB_REF_TYPE=branch`）的候选构建不受此限制；`--allow-dirty` 不会改变正式发行判定。`build-api`、`build-embedded`、`build-single`、`verify-artifacts`、`verify-single` 与候选性质的 `release-check`/`package` 在 Solid 下可用。候选包仍不可部署。

### 门禁

- **页面 payload**：`ci/release/web-bundle.sh` 按 `AUCTION_WEB_STACK` 与每个 surface 的实际技术栈构建 Dioxus 或 Solid payload；默认与显式 `dioxus` 走原来的 `dx bundle` 路径。`verify_web_payload` 对 Dioxus 保持入口页标题（“Auction 公开站点” / “Auction 管理站点”）、跨端隔离、只含普通文件与唯一一组 JS/WASM、payload 清单与哈希的现有规则；Solid 分支要求 `profile=solid-release`，入口文档不得含注释、`template`/`base`/`iframe`/`object`/`embed`，且所有 `<script>`/`<link>` 必须整体匹配真实 Vite 产物固定的四种写法（属性顺序固定、大小写敏感、双引号、同行闭合），恰好一个模块入口，图标在 Dioxus 产物中至多一个、在 Solid 产物中恰好一个；引用资源必须存在于 payload，`assets/` 文件名带 Vite 风格的 8 位内容指纹，并沿用标题判定排除另一端入口。目录遍历或摘要生成失败即拒绝复用，不把失败留下的部分输出当作有效清单。
- **体积预算**：两端 payload 的体积按 [ci/release/web-size-budget.v1](release/web-size-budget.v1) 的基线比较；Dioxus 统计 WASM 与总量，Solid 统计总量与入口首屏 JS；明显增长只告警，超过硬上限使发布失败。见 [Web payload 体积预算](#web-payload-体积预算)。
- **二进制标记**：Public 只含 Public payload 的标记与哈希，Admin 只含 Admin 的，Worker 与 auction-cli 不含任何页面标记；API-only 二进制含“资源缺失”提示而不含 payload。
- **依赖边界**（`cargo tree`）：Worker 不依赖任何前端 crate；Public 不依赖 Admin 前端；Admin 不依赖 Public 前端；`auction-server` 不依赖 `auction-cli`。分体 API 的图按各自实际 feature 检查，合并服务按 `web_cargo_features server` 的结果检查。
- **寄售预览制品门**保留发布配方守卫、编译参数与 `BUILD-INFO` 的 feature 复核。正式发布与候选制品均使用 `release` profile，关闭 `debug_assertions`。未完成的寄售功能只在 `consignment-dev-preview` 与 `debug_assertions` 同时成立时可开启，因此上述制品即使误带预览 feature 也会拒绝 `[consignment].enabled=true`，验收无需执行各二进制读取预览身份。
- **BUILD-INFO 与校验和**：每个制品目录记录源码身份、构建输入与环境指纹、feature 和文件摘要；工具链经发布工具的字节身份绑定。任一端为 Solid 时回执另记 `web_stack_public` / `web_stack_admin`，缺省全 Dioxus 的字段与顺序保持不变。复用判定比较这些指纹、锁文件、feature、两端技术栈和制品摘要；仅仓库提交或不在构建输入范围内的文档变化，不必重新编译。
- **已发布迁移**：发布 tag 中存在的 `migrations/postgres/*.up.sql` 必须在当前工作树逐字节不变；修改、重命名或删除已发布文件使 `published-migrations` 与发布工作流失败，只允许追加新编号。
- **装包**：文件白名单与权限固定（程序 755、文档 644、配置示例 640、敏感示例 600），禁止符号链接、日志与真实敏感文件名；归档确定性生成（固定排序、mtime、owner），解包复核后输出 `SHA256SUMS` 与 `DEPENDENCIES.md`。正式发布要求干净工作树（`--allow-dirty` 只用于本地证据，包名带 `-dirty`）。

发布包名为 `auction-rust-<版本>-<提交前 12 位>[-single][-dirty].tar.gz`，内容见[ops/README.md#部署形态](../ops/README.md#部署形态)。仓库 Markdown 链接相对各文档目录；装包时按同一文件安装清单重定位全部已打包文档（含 `config/README.md`），保留标题锚点，并映射敏感模板的 `.toml.example` 文件名。已打包的目标使用包内相对链接；未打包的源码、其他变体模板和文档链接到本次源码提交的 GitHub 页面，需要联网浏览。

### 版本 tag 与部署制品

只有从 `main` 上的 `v<版本>` tag 产生的 GitHub Release 资产可以部署。tag 的工作区版本必须与 `Cargo.toml` 的 `[workspace.package].version` 完全一致：

```bash
git switch main && git pull --ff-only
git tag -a v0.1.0 -m "Auction 0.1.0"    # tag 名必须等于 v + 工作区版本
git push origin v0.1.0                  # 触发 auction-release
```

tag 推送后由 `auction-release` 完成守卫、完整门禁、构建、来源证明和 Release 发布；不要手工上传资产，也不要移动或强推已发布 tag。已发布迁移文件由 `mise run verify` 中的 `published-migrations` 保护：标签所包含的任何 `*.up.sql` 都不得再修改、重命名或删除。远端还应启用 tag 保护与 Release 不可变设置（GitHub 仓库设置，不由本仓库代码强制）；发布工作流只保证同一 tag 不会产生第二个 Release。

部署端从 GitHub Release 下载 `*.tar.gz`、`SHA256SUMS`、`DEPENDENCIES.md`，然后运行：

```bash
bash ci/release/verify-deployment-package.sh auction-rust-<版本>-<12位>.tar.gz --tag v<版本>
# 或
mise run verify-package auction-rust-<版本>-<12位>.tar.gz --tag v<版本>
```

脚本失败关闭地核对：外层与包内 `SHA256SUMS`、归档成员无路径穿越/链接、`RELEASE-MANIFEST.v1` 与 `DEPENDENCIES.md` 的源码提交一致、工作树为 `clean`、构建提交可从指定发布 tag 到达，并通过 `gh attestation verify` 绑定 `release.yml` 工作流、tag 与提交的来源证明。`--offline` 只跳过来源证明，输出明确警告，不能作为部署依据。

普通 CI 的 1 天候选 Actions artifact 只用于构建核对，没有 Release 资产身份与来源证明，不满足部署条件。`auction-cli deploy` 会复用同一校验；不含该命令的版本由部署人员手工执行该脚本。

### 构建 profile

| profile | 设置 |
| --- | --- |
| `release`（后端） | 优化级别 3、fat LTO、单代码生成单元、unwind、溢出检查开启 |
| `wasm-release`（前端） | 继承溢出检查，优化级别 z、panic 为 abort |
| `dev` | `debug = "line-tables-only"`（需要完整调试信息时设 `CARGO_PROFILE_DEV_DEBUG=2`） |

release 保持 `panic = "unwind"`，不启用增量或 `target-cpu=native`。
`overflow-checks = true` 使整数溢出失败关闭，避免静默回绕。
WASM 继承开启检查，保留 abort，由 `install_panic_hook` 显示错误横幅。
`mise run verify` 不构建 release profile。

### Web payload：`ci/release/web-bundle.sh`

```bash
bash ci/release/web-bundle.sh public
bash ci/release/web-bundle.sh admin
```

入口先打印两端实际技术栈。`AUCTION_WEB_STACK` 缺省为 `dioxus`；`solid` 同时把 Public 与 Admin 切到 Solid。每次进入脚本都同时检查两端：不在支持清单里的组合会在任何清理或构建之前拒绝，即使本次只请求其中一个 surface，另一端的稳定目录也不受影响。

Dioxus 路径以 `wasm-release` profile 运行 `dx bundle`（工具由 mise 按固定版本提供），复制到稳定目录 `target/dx/build/<包名>/web/public`（后端 `web-embed` 从这里内嵌），写入 payload 清单与哈希。生成清单前依次完成四步改写：在入口 `index.html` 的脚本预加载之前声明带指纹的 WASM 预加载（`<link rel="preload" as="fetch" type="application/wasm" crossorigin>`），让 WASM 与加载器 JS 并行下载；给入口模块脚本加 `data-cfasync="false"`，让 Cloudflare Rocket Loader 不改写它（见[ops/README.md#cdncloudflare](../ops/README.md#cdncloudflare)）；把 dx 按源码路径排列的静态样式表改排为 base → surface → desktop → mobile → console（样式只由静态 head 声明，运行时不再插入第二份；同一 `@layer` 内同特异度的规则靠加载先后覆盖）；再为 `assets/` 下可压缩资源生成 brotli 副本（`*.br`）。找不到唯一的 WASM、唯一的脚本预加载锚点或唯一的模块脚本，或样式表不是恰好这五份且首尾相连时构建失败，不产出未完成改写的 payload。预压缩要求 PATH 中有 `brotli`（release 构建工作流已安装）。

Solid 路径不联网：`mise run web-prepare` 在 `bun install --frozen-lockfile` 成功后把 `web/bun.lock` 的 SHA-256 与 bun 版本写进 `web/node_modules/.auction-prepared`；构建阶段只核对这份凭据、`web/node_modules`、可执行的 `web/node_modules/.bin/vite` 与当前工具版本，不再解析安装树。凭据缺失、摘要或版本不符时提示先运行 `mise run web-prepare`，直接运行 `bun install` 不会生成凭据、构建会被拒绝。随后用本地 Vite 按统一 `AUCTION_BUILD_ID` 构建到 `target/web/<surface>.next.*` 临时目录，并固定 `NODE_ENV=production`，外部环境不能改变 `import.meta.env.DEV`；只校验入口 HTML 的唯一模块脚本带 `data-cfasync="false"`（不改写 HTML）。封存与 Dioxus 相同：brotli 副本、`profile=solid-release` 的 `auction-web-manifest.v1`、统一权限与 mtime、`WEB-BUILD-INFO`（记录 `stack=solid`、bun 版本与构建号）。新 payload 通过 `verify_web_payload` 后才换入稳定目录 `target/web/<surface>`，失败时保留旧目录。输入未变且回执的技术栈与构建号一致时直接复用，不重新运行 Vite。

生成和复核文件摘要前均检查可发布路径，发现 `*.map` 或路径中有以点开头的段即报告文件并失败。自测见 [Web 发布清单](checks/README.md#web-发布清单)。

`WEB-BUILD-INFO` 记录单端输入指纹、构建环境指纹、构建工具版本与实际编译进 payload 的构建号（`build_id`）；Dioxus 记录 dx/rustc/brotli，Solid 记录 bun/brotli 与 `stack=solid`。复用要求回执全部字段匹配，且现有 payload 再次通过完整门禁。任一端的变更使共享构建号变化时，两端都要重建。缺少该字段的旧回执同样触发重建。构建结束后重新计算单端输入指纹、环境指纹与共享构建号，全部与构建前匹配才原子发布回执。

发布入口 `ci/release/release.sh` 的 `build-api`、`build-embedded`、`build-single`、`verify-artifacts`、`verify-single`、`verify-web-size` 与 `release-check`/`package` 都接受三种组合：API-only、缺省 Dioxus、Solid（Public 与 Admin 都用 Solid）。正式发行路径见[制品变体](#制品变体)：`GITHUB_ACTIONS=true` 且 `GITHUB_REF_TYPE=tag` 的 CI 运行中，任一端为 Solid 会在任何有副作用的步骤之前以“正式发行只产出 Rust 版页面”拒绝；本地与分支上的手动候选不受限制，tag 上的手动触发同样会在安装工具前拒绝。复用原生制品前还会核对两端稳定页面目录的当前 payload 摘要，缺失或与制品回执不一致时先重建页面。`DEPENDENCIES.md` 任一端为 Solid 时附带 `web/bun.lock` 的 SHA-256 与两端技术栈，缺省输出不变。

`verify_web_payload` 的 Solid 分支要求入口文档只允许带 `src` 的模块脚本恰好一个、入口引用的资源都存在于 payload、入口恰好有一个 `<link rel="icon" href="/favicon.ico">` 且 `assets/` 下恰好有一个 `favicon-*.svg`、`assets/` 下每个文件名都带 Vite 风格的内容指纹（`<name>-<8 位 base64url 字符>.<扩展名>`，压缩副本的 `.br` 不改变此判定），profile 固定为 `solid-release`。入口 HTML 不做通用解析：出现 HTML 注释起始符（`<!` 后接 `--`）直接失败；`template`、`base`、`iframe`、`object`、`embed` 出现即失败；每个 `<script>`/`<link>` 必须整体匹配 Vite 产物固定的四种写法（属性顺序固定、大小写敏感、双引号、同行闭合），入口、样式表、预加载和图标共用这一份引用清单。单引号、无引号、未知 link 类型或无法执行的枚举命令都会失败关闭。入口模块与 `modulepreload` 声明的 JS 计入首屏 JS。

### Web payload 体积预算

```bash
bash ci/release/release.sh verify-web-size   # 只读既有 payload 与预算，不构建原生程序
mise run web-size                                # 同上
```

[ci/release/web-size-budget.v1](release/web-size-budget.v1) 是刻意维护的数据文件（`schema=auction-web-size-budget/1`）。Dioxus 的每个 surface 记录两对数字：

- `<surface>_wasm_baseline_bytes` / `<surface>_payload_baseline_bytes`：基线实测值。
- `<surface>_wasm_max_bytes` / `<surface>_payload_max_bytes`：硬上限。

当前数值只记录在该数据文件中。体积按原始表示统计：构建期 brotli 副本（`*.br`）不计入 `wasm`/`payload`/`initial_js`，只影响实际传输量。Dioxus 的 `payload` 取 `target/dx/build/<包名>/web/public` 下全部普通文件（含内嵌标记 `auction-web-manifest.v1`）的字节合计，`wasm` 取其中唯一一组 `auction-web-<surface>_bg*.wasm`；Solid 的 `payload` 取 `target/web/<surface>/public` 下全部普通文件，`initial_js` 取入口模块脚本与入口文档声明的 `modulepreload` JS 原始字节合计（重复路径只算一次）。测量前的 payload 必须先通过完整门禁，测量失败（目录缺失、摘要或体积无法计算）同样失败关闭。Solid 使用 `solid_<surface>_payload_*` 与 `solid_<surface>_initial_js_*` 键；Solid Public 的键已实测加入，Solid Admin 的四个键由统筹方在独占构建窗口实测后加入。

判定：超过 `baseline × (1 + warn_growth_basis_points/10000)`（当前 `warn_growth_basis_points=500`，即 5%）只打印 `WARN`，发布继续；超过 `_max_bytes` 打印实测值、基线与上限并使命令失败。检查点在 `build-embedded`/`release-check`（Web payload 生成后、链接原生程序前）、`verify-artifacts`/`verify-single`（复用制品同样受限）与独立的 `verify-web-size`。该检查只读既有文件，不改变运行时行为，也不写入基线。

刷新基线必须刻意进行：先确认体积增长来自有意的前端改动，运行 `mise run web-build`（或 `mise run build-release`）得到新 payload，再运行 `mise run web-size` 抄录对应技术栈输出的实测字节（Dioxus 为 `wasm=`/`payload=`，Solid 为 `initial_js=`/`payload=`），只更新对应 surface 的 baseline；按预期余量调整 `_max_bytes` 时不能低于 baseline。基线改动与引起增长的前端改动同批提交；预算失败应先排查体积回归，不得直接把上限抬到实测值以上来让发布通过。

### 构建身份：`ci/release/build-identity.sh`

`ci/release/release.sh` 与 `ci/release/web-bundle.sh` 共用：选择发布工具（用 `mise which` 解析后放到 `PATH` 前部，并设置 `NO_DOWNLOADS=1` 与专用的 `DX_HOME`，见[ci/tools/README.md#发布构建如何取用工具](tools/README.md#发布构建如何取用工具)）、计算源码身份指纹（已跟踪与未忽略的新文件、删除与 mode 变化）、计算前后端共享构建号、读取 BUILD-INFO、校验 Web payload，以及只读的 payload 体积测量与预算判定。未通过环境变量指定时，显式清空 Cargo 配置中的两类 rustc 包装器；环境变量指定的 `sccache` 替换为 mise 安装的版本，其他显式包装器保留并计入构建环境指纹。

`AUCTION_WEB_STACK` 缺省为 `dioxus`，两端都使用 Dioxus。选择 `solid` 时两端都使用 Solid，非法取值在加载身份脚本时失败。发布工具闭集取两端实际技术栈的并集。原生 Rust 编译的 sccache 总在工具闭集内。任一 surface 为 Solid 时加入 mise 提供的 bun（版本与二进制摘要），任一 surface 为 Dioxus 时保留原有的工具顺序。`web_surface_root` 对 Dioxus 返回 `target/dx/build/auction-web-<surface>/web`，对 Solid 返回 `target/web/<surface>`，内嵌目录都是其下的 `public/`。`print_web_stack_selection` 输出 `public` 与 `admin` 的实际技术栈，供构建入口在开始时记录。

`web_inputs_fingerprint` 按 surface 选择输入。Dioxus 覆盖本 surface 的 crate、浏览器 crate 的传递路径依赖、工作区构建配置与锁文件、Web 构建脚本和工具配方。Solid 取 `web/**`（在 `web/` 范围内排除任意层级的 `node_modules` 与 `.tmp` 目录）、共享的 `crates/auction-web-shared/{style,assets}`、对应 surface 的 `assets/` 与 `i18n/`、根 `Cargo.toml`、`mise.lock` 和相关发布脚本。`path_fingerprint` 默认不排除任何路径；Solid 的 Web 与原生输入共用同一处 `--exclude-dir` 规则。Solid 的原生构建输入与环境指纹也纳入实际技术栈。缺省与显式 `dioxus` 的身份输出相同。发布脚本自身属于 Web 与原生指纹输入，修改其内容会更新构建号并使之前的页面及原生复用回执失效。`web_build_identity` 对两栈都取合并后输入身份的前 32 位十六进制。Dioxus 的两种指纹函数共用浏览器 crate 路径依赖清单，由 [`web-build-inputs` 门禁](checks/README.md#web-输入依赖闭包) 校验。Solid 的 TypeScript 源码由 `web/**` 输入范围覆盖，依赖方向由 `web` 的包边界检查约束。

两个入口都在构建前把 `web_build_identity` 导出为 `AUCTION_BUILD_ID`。原生程序把它写进每个不可被共享缓存重放的 HTTP 响应的 `x-auction-build` 头与实时 WebSocket 首帧，浏览器把同一值编译进 payload 并与自身比较。检测到不同值时提示刷新，由用户决定何时重载。只改浏览器依赖闭包之外的后端源码、且未修改发布脚本等指纹输入时，构建号保持原值。
