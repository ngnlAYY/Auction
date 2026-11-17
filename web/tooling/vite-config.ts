// Public 与 Admin 共用的 Vite 构建配置：路径别名、构建号注入与产物目录只在这里定义。

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import solid from "@solidjs/vite-plugin";
import type { UserConfig } from "vite";

/** 两个互不依赖的浏览器应用；各自只打包自己的页面、样式与译文。 */
export type Surface = "public" | "admin";

const WEB_ROOT = fileURLToPath(new URL("..", import.meta.url));
const REPOSITORY_ROOT = resolve(WEB_ROOT, "..");

/** 构建号只允许 1–64 个 URL 安全字符，与服务端的校验口径一致。 */
const BUILD_ID_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

/** 读取根 Cargo.toml 的 workspace.package.version；前后端共用同一个版本号。 */
function workspaceVersion(): string {
  const manifest = readFileSync(resolve(REPOSITORY_ROOT, "Cargo.toml"), "utf8");
  const section = manifest.split(/^\[workspace\.package\]\s*$/m)[1]?.split(/^\[/m)[0];
  const version = section?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (version === undefined) {
    throw new Error("根 Cargo.toml 缺少 [workspace.package] version，无法确定页面构建号");
  }
  return version;
}

/** 发布构建经 AUCTION_BUILD_ID 注入构建号；未注入或非法时退回工作区版本。 */
function buildId(version: string): string {
  const injected = process.env.AUCTION_BUILD_ID;
  return injected !== undefined && BUILD_ID_PATTERN.test(injected) ? injected : version;
}

/** 生成某个 surface 的完整构建配置；产物写入 target/web/<surface>/public，由后端内嵌。 */
export function surfaceConfig(surface: Surface): UserConfig {
  const version = workspaceVersion();
  return {
    root: resolve(WEB_ROOT, surface),
    publicDir: false,
    plugins: [solid()],
    resolve: {
      alias: [
        { find: /^@auction\/contracts\//, replacement: `${resolve(WEB_ROOT, "contracts/src")}/` },
        { find: /^@auction\/web-shared\//, replacement: `${resolve(WEB_ROOT, "shared/src")}/` },
      ],
    },
    define: {
      __AUCTION_BUILD_ID__: JSON.stringify(buildId(version)),
      __AUCTION_PACKAGE_VERSION__: JSON.stringify(version),
    },
    build: {
      target: "es2023",
      outDir: resolve(REPOSITORY_ROOT, "target/web", surface, "public"),
      emptyOutDir: true,
      sourcemap: false,
      cssCodeSplit: true,
      // CSP 不放行 data: 资源，小文件也输出为带指纹的独立文件。
      assetsInlineLimit: 0,
      // 目标浏览器原生支持 modulepreload，不注入兼容脚本。
      modulePreload: { polyfill: false },
    },
  };
}
