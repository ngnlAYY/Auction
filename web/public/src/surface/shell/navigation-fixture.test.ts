// 用 Solid 编译实际外壳与页面，DOM 替身只记录已挂载元素、动态属性及事件。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { ThemeStateContext, useThemeEffects } from "@auction/web-shared/platform/theme";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import { transform } from "@solidjs/compiler";
import { children, createEffect, createRoot, flush, createComponent as mount } from "solid-js";
import { createComponent, findElement, TestElement } from "../catalog/command-scope-fixture.test";
import { InitialPublicLocationContext, initialPublicLocation } from "../query";
import { PUBLIC_ROUTES } from "../router/route";

export { createEffect as effect, createMemo as memo } from "solid-js";
export { insert, insertNode, ref, setProp } from "../catalog/command-scope-fixture.test";
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
    "../router/route.ts",
    "../board/index.tsx",
    "../board/world.tsx",
    "../catalog/command-scope.tsx",
    "../account/index.tsx",
    "../account/watches/index.tsx",
    "../reads/history/index.tsx",
    "../reads/history/filters/components.tsx",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/router/outlet.tsx",
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
  getAttribute(key: string): string | null {
    const value = this.attributes[key];
    return typeof value === "string" ? value : null;
  }
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

/** 实际 PublicShell 及其 RouteOutlet 共用同一窗口替身和路由。 */
export async function mountNavigation(initialUrl: string) {
  const { PublicShell }: typeof import(".") = await import(shellModule);
  const installed = installFakeWindow(initialUrl);
  const storage = new Map<string, string>();
  Object.assign(installed.fake, {
    isSecureContext: true,
    localStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
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
    const tree = children(() =>
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
    // 追踪实际挂载的子树，模拟 DOM 插槽消费动态子节点。
    createEffect(
      () => findElement(tree(), () => false),
      () => {},
    );
    flush();
    initial.expire();
    return { dispose, router, tree };
  });
  const find = (
    match: (element: DomElement) => boolean,
    parent?: DomElement,
  ): DomElement | null => {
    const node = findElement(
      parent ?? mounted.tree(),
      (element) => element instanceof DomElement && match(element),
    );
    return node instanceof DomElement ? node : null;
  };
  return {
    ...mounted,
    location: installed.fake.location,
    find,
    focus: () => installed.fake.dispatchEvent(new Event("focus")),
    dispose() {
      mounted.dispose();
      flush();
      installed.restore();
      if (navigatorDescriptor === undefined) Reflect.deleteProperty(globalThis, "navigator");
      else Object.defineProperty(globalThis, "navigator", navigatorDescriptor);
    },
  };
}
