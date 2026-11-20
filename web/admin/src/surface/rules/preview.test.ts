// Markdown 预览只显示公开正文，文章名称由明确的文档标题提供。

import { expect, test } from "bun:test";
import { loadRenderModule } from "../../solid-render-fixture.test";

import { textParts, withPage } from "../browser-page-fixture.test";
import { createComponent } from "../solid-dom-fixture";

test("document_title_names_the_article_and_only_public_body_is_visible", async () => {
  const { AdminRulesPreview } = await loadRenderModule<
    Pick<typeof import("./preview"), "AdminRulesPreview">
  >(new URL("./preview.tsx", import.meta.url).pathname, "AdminRulesPreview");
  await withPage(
    "/rules",
    () =>
      createComponent(AdminRulesPreview, {
        draft: {
          locale: "en",
          title: "Published document name",
          markdown: "# Removed heading\n\nVisible body",
        },
      }),
    ({ url }) => {
      throw new Error(`预览意外发出了请求 ${url}`);
    },
    ({ document, requests }) => {
      const text = textParts(document.roots);
      expect(text).toContain("Visible body");
      expect(text).not.toContain("Removed heading");
      expect(text).not.toContain("Published document name");
      expect(
        document.querySelectorAll("article").map((article) => article.getAttribute("aria-label")),
      ).toEqual(["Published document name"]);
      expect(requests).toEqual([]);
    },
  );
});
