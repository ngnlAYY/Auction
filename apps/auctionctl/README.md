# auctionctl

配置、迁移和运维命令行工具。

```bash
# 校验配置并显示解析结果（不回显凭据）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auctionctl -- check-config public

# 数据库迁移（schema 评审完成后启用）
cargo run -p auctionctl -- migrate
```

`check-config` 接受 `public` / `admin` / `worker`，输出角色、监听地址、数据库后端和缓存后端。
它只报告后端类型，不打印连接串。

## Cargo feature

`migrate` 需要对应后端的驱动：

- `postgres` → `auction-storage/postgres`
- `sqlite` → `auction-storage/sqlite`

默认不带驱动，因此默认构建下 `check-config` 只能校验 URL scheme 合法性。

## 安全约束

破坏性数据库操作必须有明确运行手册、精确目标确认和可验证备份。不要在本工具或开发脚本中加入
`DROP DATABASE` 一类命令。
