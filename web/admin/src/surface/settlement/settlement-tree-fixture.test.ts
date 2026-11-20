// 使用现有的 Admin JSX 内存运行时挂载结算与退件组件，并隔离浏览器与译文替身。

import { expect, test } from "bun:test";
import { RouterContext } from "@auction/web-shared/router/context";
import { openRouter } from "@auction/web-shared/router/test-window";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import type { JSX } from "@solidjs/web";
import { createRoot, flush } from "solid-js";

import { type AdminRequestHarness, createAdminRequestHarness } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope, preserveProperty } from "../../test-scope-fixture";
import { adminLocaleSession, installAdminTranslation } from "../admin-locale-fixture.test";
import { PendingCommandHostContext, usePendingCommandHost } from "../pending";
import {
  createComponent,
  createElement,
  findAll,
  insertNode,
  installTestDocument,
  mountNode,
  settle,
  type TestElement,
  visit,
} from "../solid-dom-fixture";

interface TreeFixture {
  readonly fake: ReturnType<typeof openRouter>["fake"];
  readonly router: ReturnType<typeof openRouter>["router"];
  readonly requests: AdminRequestHarness;
  mount(render: () => JSX.Element): void;
  find(predicate: (node: TestElement) => boolean): TestElement | null;
  elements(predicate: (node: TestElement) => boolean): TestElement[];
}

/** 在内存历史和真实 Solid owner 内运行用例；使用译文 bundle 的局部替身。 */
export async function withAdminTree(
  href: string,
  run: (tree: TreeFixture) => void | Promise<void>,
): Promise<void> {
  const { AdminSessionHostContext } = await loadRenderModule<
    Pick<typeof import("../auth/shell"), "AdminSessionHostContext">
  >(new URL("../auth/shell.tsx", import.meta.url).pathname, "AdminSessionHostContext");
  const requests = createAdminRequestHarness();
  const scope = cleanupScope();
  scope.defer(requests.cleanup);
  try {
    scope.defer(installAdminTranslation());
    const document = installTestDocument();
    scope.defer(document.restore);
    scope.defer(preserveProperty(globalThis, "window"));
    const { fake, router, disposeRouter } = openRouter(href);
    scope.defer(disposeRouter);
    const { host } = requests.registerHost();
    await run({
      fake,
      router,
      requests,
      mount(render) {
        createRoot((dispose) => {
          scope.defer(dispose);
          const pendingHost = usePendingCommandHost(host);
          const tree = createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(AdminSessionHostContext, {
                value: host,
                get children() {
                  return createComponent(PendingCommandHostContext, {
                    value: pendingHost,
                    get children() {
                      return createComponent(LocaleSessionContext, {
                        value: adminLocaleSession,
                        get children() {
                          return createComponent(render, {});
                        },
                      });
                    },
                  });
                },
              });
            },
          });
          mountNode(document, tree);
        });
        flush();
      },
      find: (predicate) => document.find(predicate),
      elements: (predicate) => findAll(document.roots, predicate),
    });
  } finally {
    try {
      await settle();
    } finally {
      scope.dispose();
    }
  }
}

/** 要求生产树中确实存在目标节点。 */
export function requireElement(node: TestElement | null, label: string): TestElement {
  if (node === null) {
    throw new Error(`没有渲染 ${label}`);
  }
  return node;
}

/** 读取内存节点上原生或子行的移动卡片标签。 */
export function cardLabels(root: TestElement): Set<string> {
  const labels = new Set<string>();
  visit(root, (node) => {
    const label = node.getAttribute("data-label");
    if (label !== null) {
      labels.add(label);
    }
  });
  return labels;
}

test("结算树夹具收集嵌套标签并合并重复标签", () => {
  const card = createElement("div", { "data-label": "状态" });
  const detail = createElement("span", { "data-label": "备注" });
  insertNode(card, detail);
  insertNode(detail, createElement("span", { "data-label": "状态" }));
  insertNode(card, createElement("p"));
  expect(cardLabels(card)).toEqual(new Set(["状态", "备注"]));
});
