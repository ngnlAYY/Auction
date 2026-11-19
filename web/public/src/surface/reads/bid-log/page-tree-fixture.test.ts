// 读取页按生产 Solid 规则编译，复用离线 DOM 与 History 替身。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import { transform } from "@solidjs/compiler";
import { createEffect, type Element } from "solid-js";

import {
  createComponent,
  findElement,
  type TestElement,
} from "../../catalog/command-scope-fixture.test";
import { mountReadPage } from "../../pagination/browser-fixture";

const NAMESPACE = "public-round-page-test";
const BID_LOG = resolve(import.meta.dir, "index.tsx");
const FULFILLMENT = resolve(import.meta.dir, "../fulfillment/index.tsx");
const SOURCES = new Set([BID_LOG, FULFILLMENT, resolve(import.meta.dir, "world-filter.tsx")]);
const RUNTIME = resolve(import.meta.dir, "../../shell/navigation-fixture.test.ts");
const stripTypes = new Bun.Transpiler({ loader: "ts" });
const LOCALE: LocaleSession = {
  current: () => "en",
  selected: () => "en",
  isReady: () => true,
  error: () => null,
  bootstrap() {},
  switch() {},
};

Bun.plugin({
  name: NAMESPACE,
  setup(build) {
    build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
      path,
      namespace: NAMESPACE,
    }));
    build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
      if (!SOURCES.has(path)) throw new Error(`读取页测试未登记源码 ${path}`);
      const { code } = transform(readFileSync(path, "utf8"), {
        filename: path,
        generate: "universal",
        moduleName: RUNTIME,
      });
      const contents = stripTypes
        .transformSync(code)
        .replace(/(\bfrom |\bimport\()"([^"\n]+)"/gu, (_match, prefix, specifier: string) => {
          const dependency = Bun.resolveSync(specifier, dirname(path));
          return `${prefix}${JSON.stringify(SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency)}`;
        });
      return { contents, loader: "js" };
    });
  },
});

/** 挂载生产页面一次；告警、按钮和请求都来自同一棵响应式页面树。 */
export async function mountRoundPage(href: string, bidLog: boolean) {
  const render = await loadRoundPage(bidLog);
  let tree: Element = null;
  const mounted = mountReadPage(href, () => {
    tree = createComponent(LocaleSessionContext, {
      value: LOCALE,
      get children() {
        return createComponent(render, {});
      },
    });
    createEffect(
      () => findElement(tree, () => false),
      () => {},
    );
  });
  return {
    ...mounted,
    find: (match: (element: TestElement) => boolean) => mounted.run(() => findElement(tree, match)),
  };
}

async function loadRoundPage(bidLog: boolean): Promise<() => Element> {
  if (bidLog) {
    const { BidLog }: typeof import(".") = await import(`${NAMESPACE}:${BID_LOG}`);
    return BidLog;
  }
  const { Fulfillment }: typeof import("../fulfillment") = await import(
    `${NAMESPACE}:${FULFILLMENT}`
  );
  return Fulfillment;
}
