# 静态门禁

[ci/README.md](../README.md) · [docs/development.md](../../docs/development.md)

[ci/checks/check.sh](check.sh) 是本地与 CI 的共同入口。`all` 包含仓库契约、MSRV 与工具链版本一致、公开仓库脱敏、目录布局、配置示例、systemd 模板、翻译键、装备诊断闭集、源码计数棘轮、审计写入失败分类、模块层级、Markdown 链接、迁移兼容登记、已发布迁移不可变、Web 输入依赖闭包和 Web 回调属性检查。`web` 项（`bun run typecheck`、`bun run lint` 与 `web/` 包边界）不在 `all`/`rust` 组内，由 `mise run verify` 末尾显式运行；运行前先 `mise run web-prepare` 安装 `web/node_modules`。`web-dependency-audit` 需要联网读取公告库，同样不并入 `all` 与 `mise run verify`。入口与构型见 [ci/README.md#静态门禁cicheckschecksh](../README.md#静态门禁cicheckschecksh)。`mise run verify` 等价于 `bash ci/checks/check.sh all fmt check dependency-versions lint wasm rustdoc web`：先运行 `all`，再依次执行格式、宿主默认 feature 编译、依赖版本、宿主全 feature Clippy、Public/Admin 两组 wasm32 Clippy、rustdoc 与 `web/` 静态检查，不运行行为测试。

## 公开仓库脱敏

```bash
bash ci/checks/check.sh public-redaction
```

三类扫描都遵循 `.gitignore`，被忽略的本地运行配置（如 `ops/config/` 下的角色 TOML）、草稿与原始采集不读取；全部跑完后一次列出违规的文件与行号（地址与标记扫描另附命中值）；`rg` 自身出错时立即失败：

- 本机路径、内网地址与本机测试库名：未被忽略的文件（`Cargo.lock` 除外）不得包含 `/home/<用户>/`、`/Users/<用户>/`、192.168/16 地址或按角色命名的本机测试库名（`auction-test-` 紧跟 `public`、`admin`、`worker`、`{` 或 `*`），示例改用 `/srv`、`/var/lib` 等系统路径与 `example.com`。crate 名 `auction-test-support`、指标测试的进程名与 CI 缓存键不带这些后缀，不受影响。
- IPv4 字面量：测试夹具（`apps/*`、`crates/*` 下的 `tests/fixtures/` 与 `tests/assets-fixture/`）、`docs/`、`ops/`、`ci/`、`.github/` 与全部 Markdown 中的四段 IPv4 字面量只允许回环 127.0.0.0/8、0.0.0.0 与 RFC 5737 文档段 192.0.2.0/24、198.51.100.0/24、203.0.113.0/24。词边界按 ASCII 判定，紧贴中文的地址同样命中。源码不扫描，避免把四段版本号误报为地址；`ops/deploy/caddy/Caddyfile` 的 Cloudflare 回源网段是公开的固定配置，按文件豁免。
- 用户脚本残留：`apps/*`、`crates/*` 下 `tests/fixtures/` 中的采集页面不得含浏览器用户脚本标记（不区分大小写的 `data-aad-`、`id="aad`、`hvAutoAttack`、`PonySolver`、`id="lastEncounter"`、`<style id="jpx"`、`jpx-widget`），这些注入会公开采集账号使用的自动化工具。新增采集页面时按元素删除扩展与用户脚本注入的样式、节点、属性与标题改写，保留原生结构。

扫描范围只用搜索根与 `rg --type` 限定，`-g` 只用于排除：`rg` 的包含型 glob 会覆盖 `.gitignore`，被忽略的本地配置会被读取并在失败输出中回显。确需保留的公开固定网段只按文件豁免，并在 `check_public_redaction` 的注释里写明原因，不放宽整个目录。

## 迁移兼容登记

```bash
bash ci/checks/check.sh migration-compatibility
```

[ci/checks/migration-compatibility.pl](migration-compatibility.pl) 读取迁移文件，对编号大于 074 的文件执行词法与正则约束；001–074 不要求登记且原文件不能改写。文件名须为六位编号与 snake_case 描述，新编号自 075 连续、不得重复。

词法扫描区分行注释、嵌套块注释、单引号/转义字符串、双引号标识符及带标签的 dollar quote；只在顶层分号切分 SQL，函数与 DO 内的 INSERT 也计数但不能充当合法登记，EXECUTE 字符串中的登记表引用拒绝。普通字符串与非过程 dollar quote 中的示例不计为登记。每个新文件须恰好一次 `INSERT INTO public.schema_migration_compatibility (version, level) VALUES (十进制本版本, 'compatible' 或 'breaking');`，必须是最后一条无条件顶层 SQL。允许关键字大小写及空白变化，等级字面量保持小写；禁止 upsert、INSERT SELECT、多行 VALUES、过程内登记或用注释/字符串冒充登记。

禁止 `-- no-transaction`，顶层 BEGIN/START TRANSACTION、COMMIT/END、ROLLBACK/ABORT、PREPARE/SET TRANSACTION，保持 SQLx 对迁移与登记的事务边界。失败输出文件与原因并退出 1。该门禁不执行 SQL、不连接数据库，因此不证明 compatible 的业务承诺；分类审阅规则由[migrations/postgres/README.md#兼容分级与登记](../../migrations/postgres/README.md#兼容分级与登记)拥有，`compatible` 的跨版本验证由 `mise run test-db-compat` 在真实 PostgreSQL 18 上执行，见[docs/development.md#测试](../../docs/development.md#测试)与`crates/auction-storage/tests/postgres/migration_compatibility.rs`。

## 已发布迁移不可变

```bash
bash ci/checks/check.sh published-migrations
```

[ci/checks/published-migrations.sh](published-migrations.sh) 把每个 `v<数字>` 发布 tag 视为已发布版本，读取该 tag 的 `migrations/postgres/*.up.sql`，要求当前工作树中同一路径仍然是普通文件、且字节内容与 tag 中的 Git blob 完全一致。修改、重命名或删除任一已发布迁移都会失败；追加新编号文件不受影响。检查只比较 Git 对象与工作树，不执行 SQL、不联网，也不修改 tag。

为实现失败关闭，检查拒绝浅克隆：`git rev-parse --is-shallow-repository` 必须为 `false`，CI 的 `docs` 作业因此使用 `fetch-depth: 0`。本地缺少 tag 时先执行 `git fetch --tags`；没有发布 tag 时输出“仅有未发布的迁移”并通过。发布工作流的 `guard` 作业在构建前独立运行这一检查。

## Web 输入依赖闭包

```bash
bash ci/checks/check.sh web-build-inputs
bash ci/checks/web-build-inputs-test.sh
```

`web-build-inputs` 从 `cargo metadata --format-version 1 --no-deps --locked --offline` 的结果读取依赖声明，以 `auction-web-public`、`auction-web-admin` 与 `auction-web-shared` 为入口，递归收集指向工作区成员的路径依赖。目标条件、可选 feature、构建和测试依赖的声明都计入。检查将闭包与两种 Web 指纹共用的清单比较，分别列出缺少与多出的依赖目录。清单不一致时退出 1。两种 Web 指纹经 `path_fingerprint` 计算，并用 `--exclude-dir web/node_modules --exclude-dir web/.tmp` 排除安装与临时目录；缺省 `path_fingerprint` 仍包含这些目录。相关实现见 [ci/checks/web-build-inputs.pl](web-build-inputs.pl) 与 [ci/release/build-identity.sh](../release/build-identity.sh)。

Cargo 只需读取工作区清单，下载缓存为空时也可运行，因此该门禁纳入 `all`。独立的回归脚本使用临时工作区验证传递依赖、指纹、复用和构建结束复核。

## Web 发布清单

```bash
bash ci/checks/web-payload-test.sh
```

自测在临时目录验证[发布清单门禁](../release/build-identity.sh)接受普通文件和压缩副本，拒绝源码映射、点文件和点目录下的文件，并检查失败时的文件路径诊断；Solid 用例还覆盖入口引用闭包、入口标签的闭集语法（固定属性顺序与同行闭合、单引号/无引号/注释/禁用标签拒绝）、唯一的图标链接与带指纹 favicon、首屏 JS 计量、体积预算键与失败诊断。

## Web 双栈构建入口

```bash
bash ci/checks/web-bundle-test.sh
```

自测覆盖 `ci/release/web-bundle.sh` 的双栈分支：从 Public 开始请求时，只要 Admin 被标成 Solid 也在清理任何稳定目录之前失败，Public 与 Admin 的哨兵都保持不变；`web/node_modules` 缺失、准备凭据缺失/摘要不符/bun 版本不符、`node_modules/.bin/vite` 缺失时分别失败并提示 `mise run web-prepare`，凭据相符的正例通过；缺省与显式 `dioxus` 得到相同回执并通过替身 `dx` 走原 `bundle` 路径；Solid 回执记录 `profile=solid-release` 与 `stack=solid`；稳定目录用精确目标整体替换；brotli 的枚举、临时清单、`stat`、压缩与副本写入失败都会传播；`ci/release/release.sh` 的构建类入口只在由 tag 触发的 CI 运行中选择 Solid 时拒绝，本地与分支上的手动候选构建可用，只读入口仍可运行。

## Web 回调属性

```bash
bash ci/checks/check.sh web-callback-props
bash ci/checks/web-callback-props-test.sh
```

检查 `auction-web-shared`、`auction-web-public` 与 `auction-web-admin` 的 `src/`。带 `#[component]` 的函数参数和带 `#[derive(Props)]` 的结构体字段拒绝 `Option<EventHandler<…>>` 与 `Option<Callback<…>>`，含限定路径、跨行与省略类型参数的写法。属性识别支持 `r#component`、`r#Props` 及嵌套 `cfg_attr` 中直接声明的这些属性，检查覆盖各构型。结构体的 `where` 子句按配对方括号跳过数组约束。

扫描源码声明并跳过注释、字符串和函数体，类型别名和宏展开需要人工审查。失败按文件和字段或参数的行号报告。

Dioxus 0.7.10 从 `None` 更新可选回调时会复制临时 VNode 的句柄，下一次 diff 释放该句柄的所有者。可省略的回调用非可选类型和属性默认值，使 Dioxus 在原属性槽内更新闭包。相关上游实现位于 `dioxus-core-macro/src/props/mod.rs:694–715` 和 `dioxus-core/src/events.rs:555–556`。

## Web 包边界

```bash
bash ci/checks/check.sh web
bash ci/checks/web-boundaries-test.sh
```

`web` 先要求 `web/node_modules` 已存在，且本地 `web/node_modules/.bin/tsc` 与 `web/node_modules/.bin/biome` 都是普通文件且可执行（指向普通文件的符号链接也可）；缺目录或任一工具时立即失败并提示 `mise run web-prepare`，门禁自身不安装依赖、不联网。随后在 `web/` 下运行 `bun run typecheck`、`bun run lint`，最后由薄壳 [`web/tooling/check-boundaries.ts`](../../web/tooling/check-boundaries.ts) 动态加载 [`check-boundaries-impl.ts`](../../web/tooling/check-boundaries-impl.ts)，用 `web/` 的 TypeScript 编译器解析语法树，扫描 `web/{contracts,shared,public,admin}/src/**/*.{ts,tsx}` 的 `import`、`export … from`、`import type`、动态 `import()` 与 `require()`。说明符判定是 [`web/tooling/boundary-rules.ts`](../../web/tooling/boundary-rules.ts) 的纯函数，可用 `bun test tooling/check-boundaries.test.ts`（在 `web/` 下）运行。入口不静态导入 TypeScript；实现或其依赖加载失败时以退出码 2 报告，避免被误当成越界。

边界规则与 [web/README.md](../../web/README.md#目录) 的目录表一致：`contracts` 不导入其他三个包；`shared` 只可导入 `contracts` 与自身；`public` 与 `admin` 互不导入；四个包都不导入 `web/tooling/**`、`web/` 里四个包之外的源码，以及仓库里 `web/` 之外的源码。跨包导入必须使用登记的导入名，经相对路径跨包同样违规。

说明符只接受闭集：相对路径（`./`、`../` 开头，或表示目录入口的 `.`、`..`）；检查器写死的 `@auction/contracts/*` 与 `@auction/web-shared/*` 两个别名；`web/package.json` 的 `dependencies`/`devDependencies` 中登记的第三方包（含子路径）；以及测试文件里的 `node:`、`bun:` 与 `bun` 内置模块。裸 `.`/`..`、含反斜杠的说明符、绝对路径、`file:`/`http:` 等 URL、`#` 包内导入、未登记的 `@auction/…` 与未登记的裸包名都按越界处理。别名展开后先把目标规范化为仓库内路径，再由目标所属的包判定；不能按说明符前缀判断。`web/tsconfig.base.json` 的 `paths` 必须恰好是两个登记别名；`web/` 下其他 tsconfig（不含 `node_modules`）只要出现 `compilerOptions.paths` 这个键（包括空对象）就以退出码 2 报告“别名只能登记在 web/tsconfig.base.json”，`web/package.json` 的 `imports` 只用于核对、非空即失败。出现表外条目、同一别名多个候选或条件映射时，检查器同样以退出码 2 报告“别名登记与检查器不一致”。

`web/` 之外的资产按归属放行：`crates/auction-web-shared/{assets,style}` 四个包都可引用，`crates/auction-web-public/{assets,i18n}` 只给 `public`，`crates/auction-web-admin/{assets,i18n}` 只给 `admin`。普通 `.css` 可直接引用；其他资产须带 `?raw` 或 `?url`，查询参数不改变目标路径的归属。Rust 源码、TypeScript 模块和 `web/` 之外的测试夹具不能借查询参数绕过。受检源码目录里出现任何符号链接（文件或目录）都以退出码 2 失败；判定通过的相对路径或别名目标如果在磁盘上存在，其 `realpath` 必须等于规范化后的词法路径，经过符号链接按越界（退出码 1）报告。

动态导入只接受字符串字面量或无插值模板字面量；带插值或非字面量参数报错（无法静态判定）。唯一例外是 [web/tooling/boundary-rules.ts](../../web/tooling/boundary-rules.ts) 的 `VIRTUAL_MODULE_TEST_FILES` 点名的几个测试夹具里的非字面量 `import()`（Bun 测试插件的虚拟模块）；新增同类夹具要把完整路径加进这张清单；其他文件的非字面量 `import()` 与所有非字面量 `require()` 都报错，调用目标外面套任意层括号不影响 `require()` 识别。新增同类夹具必须修改检查器里的点名清单。读取 `web/admin/src` 不存在时只豁免这一个已知的尚未并入情况；其他源码目录读取失败、`package.json`/tsconfig 解析失败、配置不一致、实现模块加载失败或扫描结果为空都以退出码 2 失败。退出码 1 仅表示发现越界导入。`ci/checks/web-boundaries-test.sh` 用临时仓库覆盖允许、拒绝与退出码 2 用例；启用 TypeScript 解析器的整体用例在能运行解析器的环境中执行。

## 译文门禁（i18n）

```bash
bash ci/checks/check.sh i18n
bash ci/checks/i18n-test.sh
```

逐个 surface 读取 `crates/auction-web-<surface>/i18n` 的五个 `.ftl`，再对 Rust 与 `web/` 两套实现运行下列检查。只有 `web/admin/src` 允许暂不存在；其余源码目录缺失、读取失败或没有受支持的源文件都以带目录名的诊断终止。三项源码检查共用一份先枚举、逐条核对 `rg` 退出码的清单，`web/shared/src` 与 `crates/auction-web-shared/src` 始终参与。

- 缺键：[`i18n-referenced-keys.pl`](i18n-referenced-keys.pl) 要求字面量键调用 `t!("…")` 与 `t("…")`（单引号、双引号或无插值模板字面量）指向当前 surface 的键。TypeScript 先从词法上去掉注释，跳过字符串与模板字面量的正文；模板插值 `${…}` 里的代码仍继续扫描，再提取调用。`t` 可以来自导入、解构或函数参数。非字面量键与 Rust 的现口径一样跳过，由闭集映射与字符串字面量可达性兜底。共享层的测试夹具含有不属于任何 surface 的假键，因此共享层以 `--skip-tests` 调用：跳过 `*.test.ts(x)`、独立的 Rust `tests.rs` / `tests/` 测试模块，并只去掉 Rust 文件里顶层 `#[cfg(test)] mod … { … }` 自身的范围，保留模块之后的代码与原始行号；surface 自身的测试源码仍全部扫描。
- 未引用键：[`i18n-unused-keys.pl`](i18n-unused-keys.pl) 要求 `en.ftl` 的每个键在同一 surface 的 Rust 或 TypeScript 源码字符串字面量中可达；两套实现任一处引用即可，设置表单的 `{id}-help` 拼接规则照旧。
- 变量与参数：[`i18n-placeholders.pl`](i18n-placeholders.pl) 先要求同一个键在五种语言里引用的变量集合完全相同（诊断带语言文件与行号），再核对字面量键调用传入的参数集合：少传与多传都报。变量只从 `{ $变量 }` 收集，`{"$USD"}` 是字符串字面量而不是变量。TypeScript 的完整形态只有 `t(键)` 与 `t(键, { 名字: 值, 简写名, … })`（允许尾逗号与换行）；对象里出现展开属性、计算属性名、第二实参不是对象字面量、第二实参为空或还有第三实参时，报“调用形态无法静态核对”并给出行号，不再跳过。
- 平铺子集：[`i18n-flat-subset.pl`](i18n-flat-subset.pl) 要求每条消息是单行的 `键 = 文本`；占位只允许 `{ $变量 }` 与 `{"…"}`（含 `\\`、`\"`、`\uXXXX`、`\UXXXXXX` 转义）。续行、选择表达式、属性、术语定义与引用、消息引用、函数调用、裸右花括号和空正文按文件与行号报错；注释行与空行跳过。分行与加载器一致：LF、CRLF、裸 CR 都是换行，U+2028/U+2029 留在行内并报错。这份子集必须与 [`web/shared/src/ui/i18n/resource.ts`](../../web/shared/src/ui/i18n/resource.ts) 的加载器同步；扩展加载器语法时也要扩展本检查。两边共用 [`i18n-subset-samples.json`](i18n-subset-samples.json) 的边界样本，加载器一侧由 `web/shared/src/ui/i18n/resource-subset.test.ts` 读取。`ci/checks/i18n-test.sh` 在临时仓库骨架上运行真实的 `bash ci/checks/check.sh i18n`，覆盖共享层错误、缺失目录、枚举失败与各反例的文件行号。

`ci/checks/i18n-test.sh` 用临时目录为上述每一项跑正例与反例，并核对诊断中的文件与行号；改动检查器或 `check.sh` 的 `i18n` 项后运行它。`check.sh` 的 `i18n` 项接受 `AUCTION_I18N_ROOT` 指向另一份仓库根，默认使用当前仓库；自测用它把真实入口指向临时骨架。

## unwrap/expect 门禁

根 [Cargo.toml](../../Cargo.toml) 的 workspace Clippy 将 `unwrap_used`、`expect_used` 设为 `deny`；所有成员继承统一策略。`lint` 检查 native 全工作区、全部 target 与 feature，再以 `--features web-embed` 单独检查两个 API 与合并服务，覆盖 `--all-features`（等于两端 Solid）看不到的 Dioxus 内嵌分支；`wasm` 分别检查 Public/Admin 浏览器及共享依赖，覆盖只在 `wasm32` 编译的分支；它还以 `wasm-release` profile 对两个页面 crate 运行 `cargo check` 并拒绝 rustc 告警，覆盖只在发布构型出现的告警（Clippy 规则仍只在 dev 构型检查）。构建脚本随所属成员检查；Clippy 会跳过部分宏展开，因此新增或修改本仓库宏时仍须核对定义中的 `unwrap`/`expect`，不能只依靠调用点的 Clippy 结果。

测试豁免不放在 `clippy.toml`（Clippy 文档注明该配置文件不稳定、将来可能废弃，仓库不使用它），而是标准的 lint 属性：每个 crate 根写 `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`，每个集成测试目标的根文件写 `#![allow(...)]`。该属性只在测试构建生效；`lint` 以 `--all-targets` 同时检查非测试构建，所以生产函数里的 `unwrap`/`expect` 仍会被拒绝。新增 crate 或集成测试目标时须带上同样的属性。生产代码须显式传播错误或处理 `None`；不可恢复的启动不变量也应给出可行动的错误。必要例外只允许最小范围的 `#[expect(clippy::unwrap_used, reason = "中文原因")]` 或对应 `expect_used`，不得在非测试构建里 crate 级关闭。

## 分配与写法统一

同一张 workspace lint 表另把一组低误报的 Clippy 规则设为 `warn`（门禁以 `-D warnings` 运行即为阻断）：多余的克隆与转换（`redundant_clone`、`implicit_clone`、`inefficient_to_string`、`cloned_instead_of_copied`、`manual_string_new`、`string_lit_as_bytes`、`single_char_pattern`）、急切求值与多余收集（`or_fun_call`、`needless_collect`），以及统一写法（`redundant_closure_for_method_calls`、`map_unwrap_or`、`semicolon_if_nothing_returned`）。评估后不采用：`unused_self` 与 `trivially_copy_pass_by_ref`（零大小服务类型的 `&self` 是注入与调用一致性的写法）、`unused_async`（跨 cfg 与 axum 处理器误报）、`unnecessary_wraps`（会沿 MoogleMail 目标构造器扩散到上百个调用点）、`items_after_statements`（局部常量就近说明）、`format_push_string`（改用 `write!` 后需处理 `fmt::Result`，收益只是一次临时分配）。

## 导入分组

```bash
bash ci/checks/check.sh import-groups
```

稳定版 rustfmt 只在连续的 `use` 段内排序，不负责分组。[ci/checks/import-groups.pl](import-groups.pl) 按 std/core/alloc → 外部 crate（含工作区其他 crate）→ 本 crate（`crate::`、`super::`、`self::`、本文件声明的子模块或 `as` 别名）三组分两层检查，覆盖 `apps/` 与 `crates/` 下的全部 Rust 源码（含测试）：

- 导入块：从文件头文档、`mod` 声明、空行或内联 `mod x {` 之后开始，由私有 `use`（可多行）与紧贴其上的属性组成；块内组序不回退，组间恰好一个空行、组内不留空行。`pub use` 门面、函数内 `use` 与含注释的块不检查。
- 导入区：每个模块（文件根与内联 `mod x { … }`）开头由私有 `use`、`pub(..) use`、`mod` 声明与属性组成的一段。先掩去注释与字面量再按条目切分；区内全部私有 `use` 的组序不回退，`pub(..) use` 不参与比较，也不切断前后私有 `use` 的比较；首个其他条目（`fn`、`struct`、`impl`、`const`、`macro_rules!` 等）之后不得再出现模块级私有 `use`。函数体等块内的 `use` 不受限；括号不平衡、无法切分条目时失败关闭。

## 源码计数棘轮

```bash
bash ci/checks/check.sh ratchet         # 与基线比较
bash ci/checks/check.sh ratchet-counts  # 仅输出当前计数，不修改基线
```

[ci/checks/source-ratchet.pl](source-ratchet.pl) 通过 `rg` 枚举 `apps/*/src/**/*.rs` 与 `crates/*/src/**/*.rs`（包括 `src` 根文件），遵循 Git 忽略规则，排除任意 `tests/` 与 `target/` 目录。除 `service-apps:english_message` 外，`tests.rs`、其他测试文件名和内联 `#[cfg(test)]` 模块仍计入；不解析条件编译，因此未启用的 feature/平台代码也计入。注释（含嵌套块注释）、普通/原始/字节/字符字面量不计入；只有 `service-apps:english_message` 读取消息位置上的字符串字面量正文，口径见下表。

计数按出现次数而非行数，多行写法同样计入。这是文本模式棘轮，不推断变量是否实际使用，也不展开宏或解析完整 Rust 语法。

| 基线模式 | 口径 |
| --- | --- |
| `all:#[path]` | 全部上述源码中直接的 `#[path = …]` / `#![path = …]` 属性；不含 `cfg_attr` 内嵌属性 |
| `all:use_super::*` | 全部上述源码中的 `use super::*`，以及 `use super::{…}` 花括号第一层中单独的 `*`（`use super::{a, *}` 计 1）；`use super::{a::*}` 这类子模块 glob、更深一层花括号内的 `*` 与 `use super::super::*` 不计。生产代码已由 `layout` 检查禁止，计数只剩测试模块 |
| `all:let_=_error` | 全部上述源码中的 `let _ = error;`（含 `err`、`e`）：先绑定错误再立即丢弃，等价于下面两项统计的 `_` 形态，基线固定为 0 |
| `层:Err(_)` | 单一参数为 `_` 或下划线开头标识符的 `Err(…)`；包括 `_error`、`ref _error`、`mut _error` 与 `ref mut _error`；无论位于匹配、表达式还是宏参数中均计入 |
| `层:map_err(\|_\|)` | `map_err` 的单参数闭包以 `_` 或下划线开头标识符接收错误；包括 `move`、`mut _error`；不含显式参数类型或其他模式 |
| `service-apps:english_message` | 四个服务装配根（`apps/auction-server`、`apps/auction-worker`、`apps/auction-admin-api`、`apps/auction-public-api` 的 `src/`）生产范围内的英文运维消息字面量。`tests.rs`/`*_tests.rs` 整个文件不计；文件内截止口径与 `layout` 行数统计相同，首个行首 `#[cfg(test)]`（可隔属性与注释行）紧跟内联 `mod 名 {` 之后不计。消息位置是 `.context(`、`.with_context(\|\|`（含 `move`；两者可再包一层 `format!(`）、`bail!(`、`anyhow!(`、`#[error(` 之后紧跟的字符串字面量，`ensure!(` 第一个顶层逗号之后紧跟的字符串字面量，以及 tracing 的 `trace!`/`debug!`/`info!`/`warn!`/`error!`/`event!` 顶层参数中第一个单独成项的字符串字面量（宏可带路径前缀，也可直接导入）。tracing 的 `字段 = 值`、`%字段`/`?字段`、`target:`/`parent:`/`name:` 与级别参数不单独成项，其中的字段值、target 与事件名不计。字面量去掉转义、`{{`/`}}` 与 `{…}` 格式占位后仍含 ASCII 字母、且全文不含汉字即计 1。注释中的写法、常量参数、span 宏（如 `info_span!`）与 `println!`/`eprintln!` 输出不计，因此 `auction-server ready: worker, admin, public` 这类已发布的固定输出行不受影响 |

错误模式分别统计六层：`jobs`、`application`、`http`、`storage`、`integrations` 对应同名 `crates/auction-*/src/`，`apps` 合并全部 `apps/*/src/`。注释和字面量被空白替代后再匹配，允许 token 之间存在任意空白。`service-apps:english_message` 只把注释与字符字面量换成空白，字符串换成占位符后匹配消息位置（tracing 宏按括号深度为 0 的逗号切分参数），再取回字面量正文判定；四个装配根的源码都找不到时失败关闭。规则来源是 [AGENTS.md#注释与文档](../../AGENTS.md#注释与文档) 对运行期消息语言的要求，基线已降到 0，新增消息须写中文。

基线在 [ci/checks/source-ratchet.baseline](source-ratchet.baseline)，每行一个 `层:模式 数量`，共 16 项。缺项、重复项、未知项与无效数量均失败关闭。当前计数高于基线时打印全部超出项并退出 1；低于基线时提示更新并继续通过。基线只能下调，须随清理改动一起审阅；不得为放行新增模式而上调。

更新时先生成候选，核对差异仅下降，再用候选替换基线并运行 `ratchet` 与 `mise run verify`：

```bash
ratchet_snapshot=$(mktemp)
bash ci/checks/check.sh ratchet-counts > "$ratchet_snapshot"
diff -u ci/checks/source-ratchet.baseline "$ratchet_snapshot"
# 人工核对只下降后再执行：
cp "$ratchet_snapshot" ci/checks/source-ratchet.baseline
rm "$ratchet_snapshot"
bash ci/checks/check.sh ratchet
```

## 审计写入失败分类

```bash
bash ci/checks/check.sh audit-writes
```

审计失败分类（`auction_audit_write_failures_total`、心跳 `audit_failure_count` 与 readiness）只能经 `AuditWriteResult::record_audit_failure` 进入；漏接的写入路径在影子观察期对分类与就绪门禁失明。[ci/checks/audit-writes.pl](audit-writes.pl) 扫描 `crates/auction-storage/src` 的生产源码（跳过 `tests.rs`、`*_tests.rs` 与内联 `#[cfg(test)] mod x { … }` 之后的内容），先切分注释与字符串/字符字面量，再对每个含 `INSERT INTO [public.]audit_records`（关键字大小写与空白不限，可跨行）的字符串字面量，要求它所在的执行语句内出现 `.await` 紧接 `.record_audit_failure()`。

语句边界取字面量之后第一个语句级 `;`，或离开字面量所在调用后遇到的包围块 `}`；注释中的示例不计。因此审计 INSERT 必须内联在执行语句中（可用多行原始字符串或 `concat!`），并在 `.await` 处直接接入；放进 `const`、返回未执行查询的函数或宏定义的写法无法证明已接入，一律失败关闭，需改为内联。门禁不展开宏、不解析完整 Rust 语法，也不证明错误随后被正确传播；一次也没有扫描到写入时失败，防止扫描范围失效后静默通过。

## 模块层级门禁

```bash
bash ci/checks/check.sh test-module-paths
```

模块树必须由目录层级表达。`src` 下的 `#[path = …]` 由计数棘轮 `all:#[path]`（基线 0，覆盖全部 `apps/*/src` 与 `crates/*/src`）阻断；本检查只兜底棘轮不扫描的测试树：`apps/*/tests` 与 `crates/*/tests` 下出现的 `#[path = …]` 一律失败。

模块的物理位置就是它的模块路径：有子模块的模块以 `name/mod.rs` 为入口并在其中声明同级子模块，叶子模块是单个 `name.rs`，两种写法不得为同一模块并存（`layout` 检查）。同一检查还要求生产源码单文件不超过 600 行、目录模块入口 `mod.rs` 不超过 500 行：从文件开头数到首个后接内联 `mod 名 {` 的行首 `#[cfg(test)]` 为止（两者之间可隔属性或注释行），`#[cfg(test)] use …;`、`#[cfg(test)] mod tests;` 等外置声明不截止，`tests/` 目录与 `tests.rs`、`*_tests.rs` 不计。同一范围内禁止私有 glob 导入：[ci/checks/source-layout.pl](source-layout.pl) 先掩去注释与字面量，再按整条语句（可跨行、花括号嵌套）检查非 `pub` 的 `use`，任何 `::*` 叶子或花括号内单独的 `*`（`use super::*`、`use super::{a::*, *}`、`use mapping::*`、`use self::别名::*` 等）都算私有 glob；只放行外部 crate 的 `prelude::*` 叶子（花括号内单独的 `*` 不放行）与单段枚举变体 glob（如 `use EquipmentSlot::*;`，模块级与函数内均可），`pub`/`pub(..)` 的 `use x::*` 门面再导出不受限，`pub(self)`、`pub(in self)` 按私有处理。模块直接从定义处或门面导入所需名字。平台差异用 `unix`/`unsupported` 这类同级模块经 `use … as platform;` 切换，不用 `#[path]` 把文件挂到别的父模块下。

父模块不为子模块囤积导入（同属 `layout` 检查）：[ci/checks/ancestor-imports.pl](ancestor-imports.pl) 从每个 crate 根（`apps/*/src`、`crates/*/src` 下的 `lib.rs` 与 `main.rs`，含 Web crate）按 `mod` 声明建立模块树（`name.rs` 或 `name/mod.rs`，内联模块的子文件按嵌套目录查找），展开每条 `use`（含嵌套分组、`self`、`as` 别名与函数体内的 `use`），并收集表达式中的 `super::`、多级 `super::super::` 与 `crate::` 路径（含宏中的 `$crate::`；宏体里的 `use $crate::…` 同样按 crate 根解析）。路径落到某个祖先模块中只是非 `pub` `use` 导入的名字（`pub(self)`、`pub(in self)` 按私有处理），且该模块既无同名定义、也无 `pub`、`pub(crate)`、`pub(super)`、`pub(in …)` 再导出时报“文件:行 名字 借用祖先模块 … 的私有导入”：祖先删掉或改名只服务自己的导入时，错误会出现在远处的子模块。子模块改为从定义处（`super::兄弟模块::名字`、`crate::…`）或门面导入，外部名字直接写外部 crate 路径。`#[cfg(test)]` 修饰的内联或文件模块整体不计（含其子树），`tests.rs`、`*_tests.rs` 与 `tests/` 目录也不计；与上文的行数口径不同，本检查不按首个内联测试模块截断文件，测试模块之后声明的生产模块与条目照常检查。注释与字面量中的路径不计；检查不展开宏、不求值其他 `cfg`，各平台分支的模块都按生产代码检查。`mod` 声明找不到源文件或花括号无法配对时失败关闭，不静默缩小范围。另外，输入中除测试文件与 `#[cfg(test)]` 声明的子树外，每个源文件都必须能从某个 crate 根经 `mod` 声明到达；到达不了的孤儿文件（不参与编译的死代码）或未被解析的声明同样失败。

`auction-application` 的 `ports/` 与 `services/` 已按限界上下文分目录，公开 API 由 crate 根部的同名上下文模块（`admin`、`auction`、`mail`、`settlement` 等）统一导出，不得再使用扁平路径或 `#[path]`。

## Markdown 相对链接

```bash
bash ci/checks/check.sh markdown-links
```

[ci/checks/markdown-links.pl](markdown-links.pl) 检查仓库全部未被忽略的 `.md` 文件（扩展名大小写均可，包含隐藏目录中的文档），排除 `.git/` 与任意 `target/`。支持行内链接、图片及已定义的完整/折叠/快捷引用链接；目标按所在文档目录解析，检查文件或目录是否存在。查询串不参与路径检查，路径和锚点的百分号编码按 UTF-8 解码。围栏/缩进代码块、行内代码及 HTML 注释中的示例不作为链接。

Markdown 同页与跨页锚点按 GitHub 标题规则生成：ATX（`#`）或 Setext（下划线）标题先去掉链接目标、HTML 与行内格式标记，保留可见文本，再转小写、去标点和符号、空白转 `-`；保留中文、下划线与连字符。同名标题追加 `-1`、`-2`，遇到已有同名锚点继续递增避让。支持显式 HTML `a` / `h1`–`h6` 的 `id` / `name`。目录链接带锚点时检查其中的 `README.md`；非 Markdown 文件仅接受存在的 GitHub `#L行号` / `#L起始-L结束` 锚点。

不访问网络：`http(s)://`、`mailto:` 及其他 URI scheme、`//` 协议相对地址与 `/` 站点绝对路径均跳过。失败输出包含来源文件、行号、原目标及缺失路径或锚点；不维护断链忽略清单。移动或重命名文档时须同步修复链接，正确目标不明确时交由维护者决定。

同一检查拒绝表格续行：以 `|` 开头的表格行之后紧跟的非空行，若不是另一表格行、也不另起块（ATX 标题、引用、HTML、列表项、分隔线），会被 GitHub 渲染成表格的一行，失败输出文件与行号，表格后须空一行。代码块、行内代码与 HTML 注释先掩为空白，不参与判断。

## 存储端口覆盖率报告（不并入 `mise run verify`）

```bash
bash ci/checks/check.sh port-coverage   # 输出覆盖率与未覆盖方法清单
```

[ci/checks/port-coverage.pl](port-coverage.pl) 用于统计存储端口方法覆盖率：在 `crates/auction-storage/src` 中找出所有 `impl <端口 trait> for Database*` 实现块（跳过测试模块），取块顶层的 `fn` 名作为方法总数；再在 `crates/auction-storage/tests/postgres/` 中按方法名查找调用（`.name(` 或 `::name(`），出现即计为覆盖。注释与字符串字面量在匹配前去除。

口径是近似上界：同名方法会被一并计为覆盖，且只说明方法至少被调用过一次，不说明分支或失败路径已被覆盖。报告只输出、不作门禁，不会让任何命令失败；用于观察扩展进度并挑选下一批待补的端口。

## 依赖升级日历（不并入 `mise run verify`）

```bash
perl ci/checks/dependency-cadence.pl [--today YYYY-MM-DD] [--file 日历路径]
```

[dependency-cadence.pl](dependency-cadence.pl) 读取 [ops/dependencies/upgrade-windows.json](../../ops/dependencies/upgrade-windows.json)，离线校验季度窗口日历：`version`、`lead_days`、窗口 ID 与状态闭集、UTC 日期合法性、季度连续、默认“第二个周一至周五”形态（顺延须填 `shifted_reason`）、与冻结区间不重叠，并要求始终存在下一次 `scheduled` 窗口。窗口已逾期仍为 `scheduled`、日历结构损坏或缺少下一次窗口时退出 1；提前 14 天或窗口进行中只输出 warning。成功时输出窗口表与门禁提示，供 CI 作业摘要使用。

本检查刻意不加入 `all` 与 `mise run verify`：日历逾期不应阻塞无关的 PR。它由[.github/workflows/dependency-cadence.yml](../../.github/workflows/dependency-cadence.yml)每周一 **05:07 UTC** 定时运行，规则见[ops/dependencies.md](../../ops/dependencies.md)。脚本只依赖 Perl 核心模块（`JSON::PP`、`Time::Local`），不联网、不修改文件、不创建 issue。

## 每周依赖审计

[.github/workflows/ci.yml](../../.github/workflows/ci.yml) 每周一 **04:17 UTC** 调度现有 `security-audit` 作业，复用 mise 安装的 cargo-audit、bun、工具链与缓存。调度事件执行 `dependency-audit`，读取默认分支 `Cargo.lock` 并获取最新 RustSec 公告；漏洞及警告令作业失败，不修改依赖、不自动发布。该作业同时执行 `web-dependency-audit`，读取 `web/bun.lock` 并查询 npm 公告源；两项相互独立。push / PR / 手动事件继续保留原有门禁与未使用依赖检查。

部署只使用发布 tag 产出的 Release 资产，默认分支已修复的公告不代表线上程序安全，因此定时与手动运行还执行 `release-dependency-audit`（[ci/checks/release-dependency-audit.sh](release-dependency-audit.sh)）：按发布守卫的 `v<semver>` 口径取最高版本 tag（`versionsort.suffix=-`，预发布排在同版本正式发布之前），把该提交的 `Cargo.lock` 读到临时目录，以 `cargo audit --deny warnings` 审计；没有发布 tag 时输出提示并通过，tag 无法解析或缺少 `Cargo.lock` 时失败。该步骤在默认分支审计失败后仍执行，两项结果分别可见；为取得 tag 的提交对象，这两类运行的 checkout 使用完整历史。push / PR 不运行此项，避免旧发布的新公告阻塞无关提交；发布工作流本身不替代周期审计。

## Web 依赖审计（不并入 `mise run verify`）

```bash
bash ci/checks/check.sh web-dependency-audit
```

该检查在 `web/` 下运行 `bun audit`，以 `web/bun.lock` 为准查询已知漏洞；需要 `web/package.json`，不要求预装 `web/node_modules`。`bun` 由 mise 提供，脚本发现缺少工具或锁文件时直接失败。`bun audit` 需要访问 npm 公告源：离线、镜像不可达或发现漏洞都以退出 1 结束，不能把“没有联网数据”当作通过。

CI 的 `security-audit` 作业在 Cargo 审计之后独立执行本项（任一失败不阻止另一项），不自动修改依赖或提交升级。安装依赖与版本升级仍走 `mise run web-prepare` 与常规门禁。
