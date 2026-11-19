// 用 Solid 编译实际外壳与页面，DOM 替身只记录已挂载元素、动态属性及事件。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { ThemeStateContext, useThemeEffects } from "@auction/web-shared/platform/theme";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import { transform } from "@solidjs/compiler";
import {
  children,
  createEffect,
  createRoot,
  type Element,
  flush,
  createComponent as mount,
} from "solid-js";
import { createComponent, findElement, TestElement } from "../catalog/command-scope-fixture.test";
import { InitialPublicLocationContext, initialPublicLocation } from "../query";
import { PUBLIC_ROUTES } from "../router/route";

export { createEffect as effect, createMemo as memo } from "solid-js";
export { insert, insertNode, setProp } from "../catalog/command-scope-fixture.test";
export { createComponent };

const NAMESPACE = "public-shell-test";
const SOURCES = new Set(
  [
    "./index.tsx",
    "./providers.tsx",
    "./view.tsx",
    "../presentation/console.tsx",
    "../presentation/index.tsx",
    "../presentation/mobile.tsx",
    "../session/index.ts",
    "../session/menu.tsx",
    "../router/index.tsx",
    "../router/route.ts",
    "../pages/status.tsx",
    "../board/index.tsx",
    "../board/data.tsx",
    "../board/world.tsx",
    "../board/navigation.tsx",
    "../catalog/actions.tsx",
    "../catalog/columns/index.tsx",
    "../catalog/command-scope.tsx",
    "../catalog/filters.tsx",
    "../catalog/rows.tsx",
    "../catalog/detail/bid-history.tsx",
    "../catalog/detail/drawer.tsx",
    "../catalog/detail/entry.tsx",
    "../catalog/detail/lookup.tsx",
    "../catalog/detail/deep-link.tsx",
    "../catalog/tables/index.tsx",
    "../catalog/tables/live.tsx",
    "../catalog/tables/preview.tsx",
    "../catalog/toast.tsx",
    "../commands/bid.tsx",
    "../commands/notice.tsx",
    "../commands/pending.tsx",
    "../commands/retry.tsx",
    "../commands/proxy/index.tsx",
    "../commands/proxy/create.tsx",
    "../commands/proxy/authorization.tsx",
    "../session/login.tsx",
    "../watches/index.tsx",
    "../watches/commands.tsx",
    "../watches/control.tsx",
    "../account/index.tsx",
    "../account/watches/index.tsx",
    "../session-preference/index.tsx",
    "../watch-preference.tsx",
    "../reads/history/index.tsx",
    "../reads/history/filters/components.tsx",
    "../reads/bid-log/index.tsx",
    "../reads/bid-log/table.tsx",
    "../reads/fulfillment/index.tsx",
    "../reads/invalid-auction.tsx",
    "../compensation/index.tsx",
    "../compensation/table.tsx",
    "../blacklist.tsx",
    "../pagination/index.tsx",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/router/outlet.tsx",
    "../../../../shared/src/ui/components/async-state.tsx",
    "../../../../shared/src/ui/components/dialog.tsx",
    "../../../../shared/src/ui/components/pager/index.tsx",
    "../../../../shared/src/ui/components/truncated-text/index.tsx",
    "../../../../shared/src/ui/i18n/index.tsx",
  ].map((path) => resolve(import.meta.dir, path)),
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
      if (!SOURCES.has(path)) {
        throw new Error(`外壳测试未登记源码 ${path}`);
      }
      const source = readFileSync(path, "utf8");
      const code = path.endsWith(".tsx")
        ? transform(source, { filename: path, generate: "universal", moduleName: import.meta.path })
            .code
        : source;
      const contents = stripTypes
        .transformSync(code)
        .replace(/(\bfrom |\bimport\()"([^"\n]+)"/gu, (_matched, prefix, specifier) => {
          const dependency = Bun.resolveSync(specifier, dirname(path));
          const target = SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency;
          return `${prefix}${JSON.stringify(target)}`;
        });
      return { contents, loader: "js" };
    });
  },
});

export class DomElement extends TestElement {
  scrollWidth = 0;
  clientWidth = 0;

  getAttribute(key: string): string | null {
    const value = this.attributes[key];
    return typeof value === "string" ? value : null;
  }

  focus(): void {
    this.attributes.focused = true;
  }

  getBoundingClientRect() {
    return { top: 100, bottom: 120, left: 20 };
  }
}

export function ref(read: () => unknown, element: DomElement): void {
  const callback = read();
  if (typeof callback === "function") callback(element);
}

export function createElement(tag: string, attributes: Record<string, unknown> = {}): DomElement {
  return new DomElement(tag, attributes);
}

export function createTextNode(text: string): DomElement {
  return createElement("#text", { text });
}

export function spread(element: DomElement, sources: readonly Record<string, unknown>[]): void {
  createEffect(
    () => Object.assign({}, ...sources),
    (attributes) => {
      Object.assign(element.attributes, attributes);
    },
  );
}

const shellModule = `${NAMESPACE}:${resolve(import.meta.dir, "index.tsx")}`;
const appModule = `${NAMESPACE}:${resolve(import.meta.dir, "../router/index.tsx")}`;
const routeModule = `${NAMESPACE}:${resolve(import.meta.dir, "../router/route.ts")}`;

/** 控制首次详情分块的交付，独立于其他用例已加载的模块和路由缓存。 */
export async function deferLotPage() {
  const { PUBLIC_PAGES }: typeof import("../router/route") = await import(routeModule);
  const page = PUBLIC_PAGES.LotDetail;
  if (page?.kind !== "lazy") throw new Error("拍品详情必须经按需路由加载");
  const started = Promise.withResolvers<void>();
  const release = Promise.withResolvers<boolean>();
  let loadingStarted = false;
  Object.assign(PUBLIC_PAGES, {
    LotDetail: {
      kind: page.kind,
      async load() {
        if (!loadingStarted) {
          loadingStarted = true;
          started.resolve();
          if (!(await release.promise)) throw new Error("测试拒绝拍品路由分块");
        }
        return page.load();
      },
    },
  });
  return {
    started: started.promise,
    release: () => release.resolve(true),
    fail: () => release.resolve(false),
    restore() {
      release.resolve(true);
      Object.assign(PUBLIC_PAGES, { LotDetail: page });
    },
  };
}

function installNavigationWindow(initialUrl: string) {
  const installed = installFakeWindow(initialUrl);
  const storage = new Map<string, string>();
  if (typeof document !== "undefined") Object.assign(installed.fake, { document });
  Object.assign(installed.fake, {
    isSecureContext: true,
    localStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
    setInterval: () => 0,
    clearInterval() {},
  });
  const navigatorDescriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      locks: {
        request: async (
          name: string,
          _options: unknown,
          body: (lock: { name: string }) => unknown,
        ) => body({ name }),
      },
    },
  });
  return {
    fake: installed.fake,
    storage,
    restore() {
      installed.restore();
      if (navigatorDescriptor === undefined) Reflect.deleteProperty(globalThis, "navigator");
      else Object.defineProperty(globalThis, "navigator", navigatorDescriptor);
    },
  };
}

function observeNodes(render: () => Element) {
  const tree = children(render);
  // 追踪实际挂载的子树，模拟 DOM 插槽消费动态子节点。
  createEffect(
    () => findElement(tree(), () => false),
    () => {},
  );
  flush();
  return (match: (element: DomElement) => boolean, parent?: DomElement): DomElement | null => {
    const node = findElement(
      parent ?? tree(),
      (element) => element instanceof DomElement && match(element),
    );
    return node instanceof DomElement ? node : null;
  };
}

/** 挂载生产入口，历史替身驱动真实路由匹配与兜底。 */
export async function mountPublicApp(
  initialUrl: string,
  storage: ReadonlyMap<string, string> = new Map(),
  rejectHistoryWrites = false,
) {
  const { App }: typeof import("../router") = await import(appModule);
  const installed = installNavigationWindow(initialUrl);
  installed.fake.rejectWrites(rejectHistoryWrites);
  for (const [key, value] of storage) installed.storage.set(key, value);
  const mounted = createRoot((dispose) => ({ dispose, find: observeNodes(() => mount(App, {})) }));
  return {
    find: mounted.find,
    storage: installed.storage,
    location: installed.fake.location,
    rejectWrites: installed.fake.rejectWrites,
    go(href: string) {
      installed.fake.history.pushState(null, "", href);
      installed.fake.dispatchEvent(new Event("popstate"));
      flush();
    },
    back() {
      installed.fake.history.back();
      flush();
    },
    forward() {
      installed.fake.history.forward();
      flush();
    },
    focus: () => installed.fake.dispatchEvent(new Event("focus")),
    dispose() {
      mounted.dispose();
      flush();
      installed.restore();
    },
  };
}

/** 实际 PublicShell 及其 RouteOutlet 共用同一窗口替身和路由。 */
export async function mountNavigation(initialUrl: string) {
  const { PublicShell }: typeof import(".") = await import(shellModule);
  const installed = installNavigationWindow(initialUrl);
  const initial = initialPublicLocation();
  const mounted = createRoot((dispose) => {
    const router = createRouter({ routes: PUBLIC_ROUTES });
    const locale: LocaleSession = {
      current: () => "en",
      selected: () => "en",
      isReady: () => true,
      error: () => null,
      bootstrap() {},
      switch() {},
    };
    const theme = useThemeEffects();
    const find = observeNodes(() =>
      mount(LocaleSessionContext, {
        value: locale,
        get children() {
          return mount(ThemeStateContext, {
            value: theme,
            get children() {
              return mount(RouterContext, {
                value: router,
                get children() {
                  return mount(InitialPublicLocationContext, {
                    value: initial,
                    get children() {
                      return mount(PublicShell, {});
                    },
                  });
                },
              });
            },
          });
        },
      }),
    );
    initial.expire();
    return { dispose, router, find };
  });
  return {
    ...mounted,
    location: installed.fake.location,
    focus: () => installed.fake.dispatchEvent(new Event("focus")),
    dispose() {
      mounted.dispose();
      flush();
      installed.restore();
    },
  };
}
