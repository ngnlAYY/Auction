// 权利、重列、损失回执、购买确认、本人详情与寄件指引页的测试夹具：独立的 Solid 编译与挂载，不经生产路由表。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";
import { createLocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { transform } from "@solidjs/compiler";
import { children, createComponent, createEffect, createRoot, type Element, flush } from "solid-js";

import { findElement, type TestElement } from "../catalog/command-scope-fixture.test";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { ALICE } from "../shell/session-fixture";
import { DOMText, settle } from "./read-fixture.test";

export { createEffect as effect, createMemo as memo } from "solid-js";
export {
  createComponent,
  insert,
  insertNode,
  setProp,
} from "../catalog/command-scope-fixture.test";
export { createElement, createTextNode, ref, spread } from "../shell/navigation-fixture.test";
export { ALICE };

const NAMESPACE = "p7p1-render";
const SOURCES = new Set(
  [
    "./rights.tsx",
    "./relist.tsx",
    "./loss-receipt.tsx",
    "./purchase-confirm.tsx",
    "./owned-detail.tsx",
    "./deposit-instructions.tsx",
    "./pending-view.tsx",
    "./detail.tsx",
    "./read-view.tsx",
    "./filters.tsx",
    "./private-page.tsx",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/ui/components/async-state.tsx",
    "../../../../shared/src/ui/components/dialog.tsx",
    "../../../../shared/src/ui/components/time.tsx",
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
      if (!SOURCES.has(path)) throw new Error("未登记的权利页测试组件");
      const source = readFileSync(path, "utf8").replace(/^import "[^"\n]+\.css";$/gmu, "");
      const code = path.endsWith(".tsx")
        ? transform(source, { filename: path, generate: "universal", moduleName: import.meta.path })
            .code
        : source;
      const contents = stripTypes
        .transformSync(code)
        .replace(/(\bfrom |\bimport\()"([^"\n]+)"/gu, (_match, prefix, specifier) => {
          const dependency = Bun.resolveSync(specifier, dirname(path));
          return `${prefix}${JSON.stringify(SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency)}`;
        });
      return { contents, loader: "js" };
    });
  },
});

/** 编译四个页面及其模板子树。 */
export async function loadPages() {
  const load = async (file: string) => {
    const entry = `${NAMESPACE}:${resolve(import.meta.dir, file)}`;
    return import(entry);
  };
  const { RightsPage }: typeof import("./rights") = await load("./rights.tsx");
  const { RelistPage }: typeof import("./relist") = await load("./relist.tsx");
  const { LossReceiptPage }: typeof import("./loss-receipt") = await load("./loss-receipt.tsx");
  const { PurchaseConfirm }: typeof import("./purchase-confirm") =
    await load("./purchase-confirm.tsx");
  const { OwnedDetail }: typeof import("./owned-detail") = await load("./owned-detail.tsx");
  const { DepositInstructionsPage }: typeof import("./deposit-instructions") = await load(
    "./deposit-instructions.tsx",
  );
  return {
    RightsPage,
    RelistPage,
    LossReceiptPage,
    PurchaseConfirm,
    OwnedDetail,
    DepositInstructionsPage,
  };
}

/** 会话存储替身。 */
export function sessionMemory() {
  return new Map<string, string>();
}

/** 挂载页面；用户为空表示未登录，权利页不依赖登录。 */
export async function mountPage(
  initialUrl: string,
  render: () => Element,
  user: typeof ALICE | null = ALICE,
  storage = sessionMemory(),
) {
  const ftl = await Bun.file(
    `${import.meta.dir}/../../../../../crates/auction-web-public/i18n/en.ftl`,
  ).text();
  const bundle = buildBundle("en", ftl, "en", null);
  if (!bundle.ok) throw new Error("权利页夹具加载译文失败");
  const installed = installFakeWindow(initialUrl);
  Object.assign(installed.fake, {
    isSecureContext: false,
    sessionStorage: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  });
  const before = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: Object.assign(new EventTarget(), { hidden: false }),
  });
  const session = createCell<PublicSessionResult | undefined>(ok(user));
  const owner = createCell<string | null>(user?.user_id ?? null);
  const authority: PublicSessionResource & { confirmation: () => { revision: null } } =
    Object.assign(session, {
      state: () => "ready" as const,
      restart() {},
      cancel() {},
      clear: () => session.set(undefined),
      confirmation: () => ({ revision: null }),
    });
  const page = createRoot((dispose) => {
    const locale = createLocaleSession(bundle.value, "en");
    const router = createRouter({ routes: [] });
    const tree = children(() =>
      createComponent(LocaleSessionContext, {
        value: locale,
        get children() {
          return createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(PublicSessionResourceContext, {
                value: authority,
                get children() {
                  return createComponent(PublicCommandOwnerContext, {
                    value: owner,
                    get children() {
                      return render();
                    },
                  });
                },
              });
            },
          });
        },
      }),
    );
    createEffect(
      () => DOMText(tree()),
      () => {},
    );
    flush();
    return { tree, dispose };
  });
  const find = (match: (node: TestElement) => boolean) => findElement(page.tree(), match);
  return {
    find,
    storage,
    text: () => DOMText(page.tree()),
    button: (text: string) => find((node) => node.tag === "button" && DOMText(node) === text),
    input: (type: string) => find((node) => node.tag === "input" && node.attributes.type === type),
    form: (text: string) => find((node) => node.tag === "form" && DOMText(node).includes(text)),
    dispose() {
      page.dispose();
      flush();
      if (before === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", before);
      installed.restore();
    },
  };
}

export { settle };
