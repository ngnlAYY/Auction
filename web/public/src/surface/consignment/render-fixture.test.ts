// 寄售页的 Solid 编译和 DOM 替身，保留原有响应式作用域与实际事件绑定。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { transform } from "@solidjs/compiler";
import { createEffect } from "solid-js";

import type { TestElement } from "../catalog/command-scope-fixture.test";

export { createEffect as effect, createMemo as memo } from "solid-js";
export {
  createComponent,
  createElement,
  createTextNode,
  insert,
  insertNode,
  ref,
  setProp,
} from "../catalog/command-scope-fixture.test";

const NAMESPACE = "public-consignment-render";
const pageChunks = new Map(
  ["catalog", "detail", "registration", "owned", "owned-detail"].map((page) => [
    resolve(import.meta.dir, `./${page}.tsx`),
    page,
  ]),
);
const pageChunkRequests: string[] = [];
const SOURCES = new Set(
  [
    "./capability.ts",
    "./filters.tsx",
    "./pending-view.tsx",
    "./private-page.tsx",
    "./read-view.tsx",
    "../presentation/index.tsx",
    "../presentation/console.tsx",
    "../presentation/mobile.tsx",
    "../router/route.ts",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/router/outlet.tsx",
    "../../../../shared/src/ui/components/async-state.tsx",
    "../../../../shared/src/ui/components/dialog.tsx",
    "../../../../shared/src/ui/components/pager/index.tsx",
    "../../../../shared/src/ui/components/pager/size.tsx",
    "../../../../shared/src/ui/components/time.tsx",
  ]
    .map((path) => resolve(import.meta.dir, path))
    .concat([...pageChunks.keys()]),
);
const stripTypes = new Bun.Transpiler({ loader: "ts" });

Bun.plugin({
  name: NAMESPACE,
  setup(build) {
    build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
      path,
      namespace: NAMESPACE,
    }));
    build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
      if (!SOURCES.has(path)) throw new Error("寄售测试组件尚未登记");
      const source = readFileSync(path, "utf8").replace(/^import "[^"\n]+\.css";$/gmu, "");
      const code = path.endsWith(".tsx")
        ? transform(source, { filename: path, generate: "universal", moduleName: import.meta.path })
            .code
        : source;
      const contents = stripTypes
        .transformSync(code)
        .replace(
          /\bfrom "([^"\n]+)"|\bimport\("([^"\n]+)"\)/gu,
          (_match, staticSpecifier, dynamicSpecifier) => {
            const dependency = Bun.resolveSync(staticSpecifier ?? dynamicSpecifier, dirname(path));
            const target = JSON.stringify(
              SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency,
            );
            if (staticSpecifier !== undefined) return `from ${target}`;
            const page = pageChunks.get(dependency);
            if (page === undefined) return `import(${target})`;
            return `observePageChunk(${JSON.stringify(page)}, () => import(${target}))`;
          },
        );
      return {
        contents: `import { observePageChunk } from ${JSON.stringify(import.meta.path)};\n${contents}`,
        loader: "js",
      };
    });
  },
});

/** 编译真实详情及其模板子树。 */
export async function loadDetail() {
  const entry = `${NAMESPACE}:${resolve(import.meta.dir, "./detail.tsx")}`;
  const module: typeof import("./detail") = await import(entry);
  return module;
}

/** 编译真实目录及其模板子树。 */
export async function loadCatalog() {
  const entry = `${NAMESPACE}:${resolve(import.meta.dir, "./catalog.tsx")}`;
  const module: typeof import("./catalog") = await import(entry);
  return module;
}

/** 编译导航和路由出口，但不预先执行寄售页面加载器。 */
export async function loadNavigation() {
  const nav = `${NAMESPACE}:${resolve(import.meta.dir, "../presentation/index.tsx")}`;
  const consoleNav = `${NAMESPACE}:${resolve(import.meta.dir, "../presentation/console.tsx")}`;
  const mobileNav = `${NAMESPACE}:${resolve(import.meta.dir, "../presentation/mobile.tsx")}`;
  const outlet = `${NAMESPACE}:${resolve(import.meta.dir, "../../../../shared/src/router/outlet.tsx")}`;
  const { NavigationLinks }: typeof import("../presentation") = await import(nav);
  const { ConsoleSidebar }: typeof import("../presentation/console") = await import(consoleNav);
  const { Navigation }: typeof import("../presentation/mobile") = await import(mobileNav);
  const { RouteOutlet }: { RouteOutlet: typeof import("@auction/web-shared/router").RouteOutlet } =
    await import(outlet);
  const route = `${NAMESPACE}:${resolve(import.meta.dir, "../router/route.ts")}`;
  const { PUBLIC_PAGES }: typeof import("../router/route") = await import(route);
  return { NavigationLinks, ConsoleSidebar, Navigation, RouteOutlet, PUBLIC_PAGES };
}

/** 保留 Link 透传属性的响应式更新。 */
export function spread(element: TestElement, sources: readonly Record<string, unknown>[]): void {
  createEffect(
    () => Object.assign({}, ...sources),
    (attributes) => {
      Object.assign(element.attributes, attributes);
    },
  );
}

/** 每次动态导入请求都计数，包括已经在 Bun 缓存中的页面。 */
export function observePageChunk<T>(page: string, load: () => Promise<T>): Promise<T> {
  pageChunkRequests.push(page);
  return load();
}

/** 返回截至当前的寄售页面动态导入请求快照。 */
export function requestedPageChunks(): readonly string[] {
  return [...pageChunkRequests];
}
