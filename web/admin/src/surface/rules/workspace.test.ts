// 规则工作区在发布体过大后展示纠正提示，正文仍可编辑。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { SESSION_REPLY, success } from "../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  byId,
  click,
  inputAndExpectValueUpdate,
  textParts,
  withPage,
} from "../browser-page-fixture.test";
import { valueUpdates } from "../solid-dom-fixture";

test("oversized_publish_explains_how_to_correct_the_draft", async () => {
  const { WorkspaceHarness } = await loadRenderModule<
    Pick<typeof import("./document-fixture"), "WorkspaceHarness">
  >(new URL("./document-fixture.tsx", import.meta.url).pathname, "WorkspaceHarness");
  await withPage(
    "/rules",
    WorkspaceHarness,
    ({ method, url, body }) => {
      const request = `${method} ${url.pathname}${url.search}`;
      if (request === "GET /api/v1/admin/session?include=server_time") {
        return SESSION_REPLY;
      }
      expect(request).toBe("PUT /api/v1/admin/rules");
      expect(success(parseJson(body))).toEqual({
        command_id: expect.any(String),
        expected_revision: 1,
        locale: "zh-CN",
        title: "原规则",
        markdown: "更新的正文",
      });
      return {
        status: 413,
        body: '{"code":"body_too_large","message":"请求体过大","data":null,"request_id":null}',
      };
    },
    async ({ document, requests }) => {
      const texts = () => textParts(document.roots);
      const error = "请求体过大。请缩短内容后重试。";
      expect(texts()).not.toContain(error);
      expect(texts()).toContain("保存并发布");
      expect(requests).toEqual([]);
      const markdown = byId(document, "rules-markdown");
      expect(markdown.value).toBe("更新的正文");
      expect(valueUpdates(markdown)).toContain("更新的正文");
      await click(document, "保存并发布");
      expect(requests.map(({ method, url }) => `${method} ${url.pathname}${url.search}`)).toEqual([
        "GET /api/v1/admin/session?include=server_time",
        "PUT /api/v1/admin/rules",
      ]);
      expect(texts()).toContain(error);
      await inputAndExpectValueUpdate(markdown, "短正文");
      expect(byId(document, "rules-markdown")).toBe(markdown);
      expect(markdown.value).toBe("短正文");
    },
  );
});
