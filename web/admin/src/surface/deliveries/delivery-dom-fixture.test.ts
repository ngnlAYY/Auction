// 为投递与邮件组件装配路由、认证和请求替身，DOM 使用 Admin 共用实现。

import { expect, test } from "bun:test";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import { createRoot, type Element, flush, untrack } from "solid-js";
import { createAdminRequestHarness } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { adminLocaleSession, installAdminTranslation } from "../admin-locale-fixture.test";
import { PendingCommandHostContext, usePendingCommandHost } from "../pending";
import { ADMIN_ROUTES } from "../router/route";
import {
  createComponent,
  createElement,
  createTextNode,
  insert,
  insertNode,
  installTestDocument,
  TestDocument,
  type TestElement,
} from "../solid-dom-fixture";

/** 推进响应式事务与内存请求的微任务。 */
export { settle } from "../solid-dom-fixture";

/** 投递与邮件测试加载已登记的真实组件。 */
export const DELIVERY_RENDER = {
  detail: () =>
    loadRenderModule<typeof import("./detail")>(
      new URL("./detail/index.tsx", import.meta.url).pathname,
      "detailPage",
    ),
  detailWithList: () =>
    loadRenderModule<typeof import("./detail/detail-page-fixture")>(
      new URL("./detail/detail-page-fixture.tsx", import.meta.url).pathname,
      "DetailWithListCommand",
    ),
  dialog: () =>
    loadRenderModule<typeof import("./dialog")>(
      new URL("./dialog.tsx", import.meta.url).pathname,
      "DeliveryDialog",
      "DeliveryEvidenceStatus",
    ),
  filters: () =>
    loadRenderModule<typeof import("./filters")>(
      new URL("./filters.tsx", import.meta.url).pathname,
      "DeliveryFilters",
    ),
  sentProof: () =>
    loadRenderModule<typeof import("./sent-proof/render")>(
      new URL("./sent-proof/render.tsx", import.meta.url).pathname,
      "SentProofDialog",
    ),
  sentPreview: () =>
    loadRenderModule<typeof import("./sent-proof/preview")>(
      new URL("./sent-proof/preview.tsx", import.meta.url).pathname,
      "runPreview",
    ),
  mailDeliveries: () =>
    loadRenderModule<typeof import("../mail/deliveries-fixture")>(
      new URL("../mail/deliveries-fixture.tsx", import.meta.url).pathname,
      "LinkedDeliveryFixture",
    ),
  mailPolling: () =>
    loadRenderModule<typeof import("../mail/polling-fixture")>(
      new URL("../mail/polling-fixture.tsx", import.meta.url).pathname,
      "PollingActionsFixture",
      "MailPageFixture",
    ),
};

/** 投递页测试按完整文案、按钮或固定 ID 查询共用的内存节点。 */
export class DeliveryDocument extends TestDocument {
  readonly root = createElement("root");

  constructor() {
    super();
    this.add(this.root);
  }

  override find(match: (node: TestElement) => boolean): TestElement | null {
    return untrack(() => super.find(match));
  }

  need(match: (node: TestElement) => boolean): TestElement {
    const node = this.find(match);
    if (node === null) throw new Error(`找不到节点：${this.root.textContent}`);
    return node;
  }

  byId(id: string): TestElement {
    return this.need((node) => node.id === id);
  }

  findButton(text: string): TestElement | null {
    return this.find((node) => node.tag === "button" && node.textContent === text);
  }

  button(text: string): TestElement {
    return this.need((node) => node.tag === "button" && node.textContent === text);
  }

  hasExactText(text: string): boolean {
    return this.find((node) => node.textContent === text) !== null;
  }
}

/** 单次挂载真实页面与会话宿主，并在用例退出时释放 DOM、路由和传输。 */
export async function withDeliveryDom(
  url: string,
  run: (dom: DeliveryDocument, mount: (component: () => Element) => void) => Promise<void> | void,
): Promise<void> {
  const { AdminSessionHostContext } = await loadRenderModule<typeof import("../auth/shell")>(
    new URL("../auth/shell.tsx", import.meta.url).pathname,
    "AdminSessionHostContext",
  );
  const scope = cleanupScope();
  try {
    scope.defer(installFakeWindow(url).restore);
    const dom = installTestDocument(new DeliveryDocument());
    scope.defer(dom.restore);
    scope.defer(installAdminTranslation());
    const requests = createAdminRequestHarness();
    scope.defer(requests.cleanup);
    const { host } = requests.registerHost();
    let mounted = false;
    await run(dom, (component) => {
      if (mounted) throw new Error("页面只能挂载一次");
      mounted = true;
      createRoot((release) => {
        scope.defer(() => {
          try {
            release();
          } finally {
            flush();
          }
        });
        const router = createRouter({ routes: ADMIN_ROUTES });
        const pending = usePendingCommandHost(host);
        const tree = createComponent(RouterContext, {
          value: router,
          get children() {
            return createComponent(AdminSessionHostContext, {
              value: host,
              get children() {
                return createComponent(PendingCommandHostContext, {
                  value: pending,
                  get children() {
                    return createComponent(LocaleSessionContext, {
                      value: adminLocaleSession,
                      get children() {
                        return component();
                      },
                    });
                  },
                });
              },
            });
          },
        });
        insert(dom.root, tree);
      });
    });
  } finally {
    scope.dispose();
  }
}

/** 直接依赖本地时区的测试在用例内固定时区并还原。 */
export async function withTestTimezone(
  zone: string,
  run: () => void | Promise<void>,
): Promise<void> {
  const previous = process.env.TZ;
  try {
    process.env.TZ = zone;
    await run();
  } finally {
    if (previous === undefined) delete process.env.TZ;
    else process.env.TZ = previous;
  }
}

test("投递文档夹具按完整可见文案查找按钮", () => {
  const document = new DeliveryDocument();
  const button = createElement("button");
  insertNode(button, createTextNode("打开投递"));
  insertNode(document.root, button);
  expect(document.button("打开投递")).toBe(button);
  expect(document.findButton("打开")).toBeNull();
  expect(() => document.button("打开")).toThrow("找不到节点");
});
