# Agent Note: PostgreSQL 连接参数独立填写

Status: implemented

## Problem

数据库配置原先要求填写完整连接串，用户需要自行处理主机、端口、账号、密码和库名的位置与
转义。含 `@`、`:` 或 `%` 的密码容易被误解释成 URL 结构。用户明确要求五项分开填写，配置文件
和首次设置页必须使用同一语义，避免页面连接检查成功但保存后的正式进程连接到不同目标。

## Decision

PostgreSQL 敏感配置使用 `[database]`，必填 `host`、`port`、`username`、`password`、`name`。
五项原文由 `auction-config` 的 `settings/secrets/database.rs` 拥有；端口必须非零，主机只接受
DNS/IP，不能携带协议、端口或路径。支持 IPv4、IPv6；不从用户名或环境默认值推导目标库。

配置层把五项转换成唯一的进程内部 URL，原有 `AppConfig::database_url()` 消费边界保持稳定。
userinfo 的字面 `%` 先转义，再由 URL setter 编码分隔符；库名作为单个路径段编码，拒绝会被
路径规范化吞掉的点段。敏感结构的 `Debug` 固定脱敏，错误只带静态字段名，不返回凭据或 URL。
因此 SQLx、部署目标比较、备份恢复和独立维护凭据共用同一转换结果。

SQLite 文件、PostgreSQL Unix socket 和显式 TLS profile 仍可通过兼容的 `database_url`
提供；它与 `[database]` 互斥，二者并存时失败关闭。五字段形式不新增 TLS 策略，沿用驱动默认；
已有显式 TLS 的运维模板保持原 URL。兼容输入不自动改写已部署文件。

首次设置契约使用 `database` 对象，字段与敏感 TOML 同名；HTTP DTO 与配置类型分离，在
`auction-setup` 显式映射。设置页五语言提供五个输入框，数据库密码成功提交后从表单清除。
校验与保存复用配置模型，落盘写出 `[database]`，不再让用户或浏览器拼接连接串。

通用 PostgreSQL 敏感模板位于 `ops/config/examples/secrets/postgres.toml`，发布归档把它装为
`config/examples/secrets.toml.example`；仓库原有 SQLite 示例继续服务 Lite 档。数据库五项之外
的管理员、邮件和数据密钥仍按原 owner 配置，不能被误装进 `[database]`。

## 既有决定审计

本次部分扩展[首次设置决定](2026-09-17-first-run-setup.md)，只更换数据库输入形状，保留独立
loopback 表面、一次性令牌、不覆盖目录和不自动迁移的边界；双方互链。
[发布包范围决定](../simplification/2026-09-18-release-artifact-scope.md)继续拥有附加安装层删除，
本次仅让其通用敏感样例适配 PostgreSQL 发布档。文档重建决定只拥有门禁，没有配置语义重叠。

## Alternatives considered

1. **仅把账号密码从 URL 拆出来。** 可继续把主机、端口与库名放在一个地址中，但不满足用户
   明确要求五项独立输入，也会继续让使用者维护 URL 路径。
2. **在页面拼接 URL 再保存旧字段。** 后端改动较少，但配置文件仍没有五个独立字段，浏览器
   与配置 loader 会各自维护转义规则，因此统一在配置层转换。
3. **直接取消旧 URL。** 新模型更单一，但会中断现有 PostgreSQL/SQLite 配置，并失去已有
   Unix socket/TLS 表达能力；保留互斥的兼容入口，不静默选择或弱化旧连接策略。

## Consequences

用户填写五项原文即可，密码中的分隔符无需手工转义；正式进程、首次设置和运维工具使用同一
连接结果。代价是配置层需要维护新旧输入互斥与编码边界，初始化请求的旧 URL 字段不再接受，
必须随 Admin Web 与后端一同发布。自定义 TLS/Unix socket 暂时使用旧 URL，若要扩展结构化
连接参数，应另行冻结对应字段和维护库一致性规则。

验证使用 `just verify`、配置示例/i18n/文档门禁与完整发布构建，并静态核对特殊字符编码、
五字段必填和冲突失败路径。遵守仓库范围，不新增或运行行为测试，也不连接真实数据库。
