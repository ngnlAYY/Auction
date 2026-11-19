// Public 与 Admin 共用的 Vite 构建配置：路径别名、构建号注入与产物目录只在这里定义。

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import solid from "@solidjs/vite-plugin";
import type { Plugin, UserConfig } from "vite";

/** 两个互不依赖的浏览器应用；各自只打包自己的页面、样式与译文。 */
export type Surface = "public" | "admin";

const WEB_ROOT = fileURLToPath(new URL("..", import.meta.url));
const REPOSITORY_ROOT = resolve(WEB_ROOT, "..");

/** 构建号只允许 1–64 个 URL 安全字符，与服务端的校验口径一致。 */
const BUILD_ID_PATTERN = /^[A-Za-z0-9._-]{1,64}$/;

const HTML_COMMENT = /<!--[\s\S]*?(?:-->|$)/g;
const SCRIPT_TAG = /<script\s[^>]*>/gi;
const MODULE_TYPE = /\stype\s*=\s*(?:"module"|'module'|module)(?=\s|>)/i;
const SCRIPT_SOURCE = /\ssrc="[^"]+"/;
const CF_ASYNC_ATTRIBUTE = /\sdata-cfasync(?=[\s=>])/gi;
const ROCKET_LOADER_OPT_OUT = 'data-cfasync="false"';
const DEV_FAVICON_PATH = "/favicon.ico";
const DEV_FAVICON_SOURCE = resolve(REPOSITORY_ROOT, "crates/auction-web-shared/assets/favicon.svg");

interface DevFaviconRequest {
  method?: string | undefined;
  url?: string | undefined;
}

interface DevFaviconResponse {
  statusCode: number;
  setHeader(name: string, value: string): void;
  end(body?: Buffer): void;
}

/** 开发服务器把 /favicon.ico 映射到共享 SVG；生产由后端做同样的映射。 */
export function handleDevFavicon(
  request: DevFaviconRequest,
  response: DevFaviconResponse,
): boolean {
  const path = request.url?.split("?", 1)[0];
  if (path !== DEV_FAVICON_PATH || (request.method !== "GET" && request.method !== "HEAD")) {
    return false;
  }
  response.statusCode = 200;
  response.setHeader("Content-Type", "image/svg+xml");
  response.setHeader("Cache-Control", "no-cache");
  response.end(request.method === "HEAD" ? undefined : readFileSync(DEV_FAVICON_SOURCE));
  return true;
}

function entryModuleScript(html: string): { tag: string; offset: number } {
  // 注释里的示例脚本不会执行。等长屏蔽注释可保留入口在原 HTML 中的位置。
  const markup = html.replace(HTML_COMMENT, (comment) => " ".repeat(comment.length));
  const [entry, extra] = [...markup.matchAll(SCRIPT_TAG)].filter(([tag]) => MODULE_TYPE.test(tag));
  if (entry?.index === undefined || extra !== undefined || !SCRIPT_SOURCE.test(entry[0])) {
    throw new Error("Web 入口 HTML 必须恰好包含一个带 src 的模块脚本");
  }
  return { tag: entry[0], offset: entry.index };
}

function rocketLoaderOptOut(): Plugin {
  return {
    name: "auction-rocket-loader-opt-out",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(html) {
        // Vite 会重建入口标签，Rocket Loader 要求豁免属性出现在 src 之前。
        const { tag, offset } = entryModuleScript(html);
        if (tag.match(CF_ASYNC_ATTRIBUTE) !== null) {
          return html;
        }
        const exempt = tag.replace(/^<script/i, `<script ${ROCKET_LOADER_OPT_OUT}`);
        return html.slice(0, offset) + exempt + html.slice(offset + tag.length);
      },
    },
    generateBundle: {
      order: "post",
      handler(_options, bundle) {
        const index = bundle["index.html"];
        if (index?.type !== "asset" || typeof index.source !== "string") {
          this.error("Web 构建缺少最终入口 HTML");
        }
        const { tag: entry } = entryModuleScript(index.source);
        const beforeSource = entry.slice(0, entry.search(SCRIPT_SOURCE)).split(/\s+/);
        if (
          entry.match(CF_ASYNC_ATTRIBUTE)?.length !== 1 ||
          !beforeSource.includes(ROCKET_LOADER_OPT_OUT)
        ) {
          this.error("Web 入口模块脚本缺少有效的 Rocket Loader 豁免");
        }
      },
    },
  };
}

/** 只在开发服务器补上生产后端提供的 /favicon.ico 映射，构建路径不挂载。 */
function devFavicon(): Plugin {
  return {
    name: "auction-dev-favicon",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (!handleDevFavicon(request, response)) {
          next();
        }
      });
    },
  };
}

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
    // 构建输入只来自仓库与 AUCTION_BUILD_ID；Vite 8 用 envDir: false 关闭 .env* 加载（envFile 已弃用）。
    envDir: false,
    plugins: [solid(), rocketLoaderOptOut(), devFavicon()],
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
