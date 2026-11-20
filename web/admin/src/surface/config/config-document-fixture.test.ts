// 装配运行配置文档的会话草稿、页面挂载与读取入口。

import { expect, test } from "bun:test";
import { createRoot } from "solid-js";
import type { WriteReply } from "../../api/api-request-fixture";
import { cleanupScope } from "../../test-scope-fixture";
import { createElement } from "../solid-dom-fixture";
import { configPage } from "./config-page-fixture.test";
import { CONFIG_PROJECTION_QUERY } from "./config-projection-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { provide } from "./config-test-scope-fixture.test";
import { type ConfigDraft, ConfigDraftContext, useConfigDraft } from "./draft";

const fixture = await loadConfigFixture();

/** 模拟认证外壳在同一页面内触发的配置重读。 */
export function refreshConfigDraft(draft: ConfigDraft): void {
  draft.refresh.update((generation) => generation + 1n);
}

/** 每次挂载真实编辑器，卸载时保留会话草稿；后台读取用例可装配 Rust 对应的测试按钮。 */
export function configDocumentPage(reply: WriteReply, rereadLabel?: string) {
  const scope = cleanupScope();
  try {
    const draft = createRoot((stop) => {
      scope.defer(stop);
      return useConfigDraft();
    });
    const page = configPage((request, init, index) => {
      const url = new URL(request.url);
      switch (request.method) {
        case "GET":
          expect(url.pathname).toBe("/api/v1/admin/config");
          expect(url.search).toBe(`${CONFIG_PROJECTION_QUERY}&include_worker_restart_state=true`);
          break;
        case "POST":
          expect(url.pathname).toBe("/api/v1/admin/config:validate");
          expect(url.search).toBe(CONFIG_PROJECTION_QUERY);
          break;
        case "PUT":
          expect(url.pathname).toBe("/api/v1/admin/config");
          expect(url.search).toBe(CONFIG_PROJECTION_QUERY);
          break;
        default:
          throw new Error(`意外请求 ${request.method} ${request.url}`);
      }
      return reply(request, init, index);
    });
    scope.defer(page.dispose);
    let unmount = () => {};
    return {
      ...page,
      draft,
      mount() {
        unmount = page.mount(() =>
          provide(ConfigDraftContext, draft, () => {
            const editor = fixture.createComponent(fixture.document.RuntimeConfigDocument, {});
            if (rereadLabel === undefined) {
              return editor;
            }
            const reread = createElement("button", { onClick: () => refreshConfigDraft(draft) });
            fixture.insert(reread, rereadLabel);
            return [reread, editor];
          }),
        );
      },
      unmount() {
        unmount();
      },
      dispose: scope.dispose,
    };
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

test("文档夹具的模拟重读每次推进同一个草稿的世代", () => {
  createRoot((dispose) => {
    try {
      const draft = useConfigDraft();
      expect(draft.refresh.peek()).toBe(0n);
      refreshConfigDraft(draft);
      expect(draft.refresh.peek()).toBe(1n);
      refreshConfigDraft(draft);
      expect(draft.refresh.peek()).toBe(2n);
    } finally {
      dispose();
    }
  });
});
