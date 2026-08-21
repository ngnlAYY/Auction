# auction-http

Public 与 Admin 的 HTTP 表面适配器。

- `lib.rs`：`public_router()` / `admin_router()`，健康检查处理器。
- `web_assets.rs`：前端静态产物内嵌与 SPA fallback。

## 职责边界

只做 HTTP 输入到应用命令、应用输出到稳定契约的映射。处理器**不得**包含竞价、结算或状态机
规则——那些属于领域与应用层。

Public 与 Admin 是不同 surface，路由和静态产物分别隔离；公开端二进制不包含管理端资源。

## 静态产物内嵌

两个 surface 各自内嵌对应的 `dx build` 产物，路径由 `rust-embed` 按本 crate 的
`CARGO_MANIFEST_DIR` 解析：

```text
target/dx/build/auction-web-public/web/public
target/dx/build/auction-web-admin/web/public
```

`allow_missing = true` 是必需的。`rust-embed` 默认在目标目录不存在时**编译期报错**，而前端
产物由独立的 `dx build` 生成；没有这个属性，首次克隆仓库、CI 静态检查或只改后端的构建都会
因为缺少产物而无法编译。缺失产物在运行时以 503 明确报告，不 panic。

release 构建把产物烘进二进制，dev 构建从磁盘读取，因此本地改前端不必重编后端。

请求命中顺序：

1. 精确匹配静态文件；
2. 无扩展名路径回退 `index.html`（对应旧 `manifests/*.routes.json` 的浏览器路由）；
3. 带扩展名但未命中按 404 处理，**不**回退 HTML——否则缺失的 `.wasm`/`.js` 会拿到一份 200
   的 HTML，浏览器端只会报难以定位的解析错误；
4. 产物整体缺失返回 503 并说明需要执行的构建命令。

入口文档使用 `no-cache`：它引用带指纹的 WASM/JS 名称，缓存住会让浏览器加载到已被替换的旧
产物。指纹化产物本身按 `immutable` 长期缓存。
