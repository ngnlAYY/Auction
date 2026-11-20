// 场次页收到同世界录入回执后，重新获取并显示最新场次快照。

import { expect, test } from "bun:test";
import { ok } from "@auction/web-shared/result";
import { formatLocalizedTimeForLanguage } from "@auction/web-shared/ui/i18n";
import { fixtureText } from "../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { createComponent } from "../solid-dom-fixture";
import { type EditorScope, present, settleEditor, withEditor } from "./editor-fixture.test";

test("auction_list_requests_a_fresh_snapshot_after_intake_settles", async () => {
  const { page } = await loadRenderModule<typeof import(".")>(
    `${import.meta.dir}/index.tsx`,
    "page",
  );
  const paths: string[] = [];
  let source: EditorScope | undefined;
  const current = fixtureText("auctions");
  const previousOpen = "2025-11-15T11:46:40.110000Z";
  const nextOpen = "2025-11-15T12:16:40.110000Z";
  const next = current.replace(previousOpen, nextOpen);
  await withEditor(
    (scope) => {
      source = scope;
      return createComponent(page, {});
    },
    async (mounted) => {
      const path = "/api/v1/admin/auctions?page=1&page_size=50";
      expect(paths).toEqual([path]);
      mounted.field("auction-world").change("main");
      await settleEditor();
      const filtered = `${path}&world_id=main`;
      expect(paths).toEqual([path, filtered]);
      const previousTime = formatLocalizedTimeForLanguage(previousOpen, "zh-CN");
      const nextTime = formatLocalizedTimeForLanguage(nextOpen, "zh-CN");
      expect(mounted.hasText(previousTime)).toBe(true);
      expect(mounted.hasText(nextTime)).toBe(false);
      const { owner } = present(source, "场次页所属会话");
      expect(
        owner.submit(
          "intake:main",
          "录入装备",
          async () => ok(null),
          () => {},
          () => {
            throw new Error("录入应成功");
          },
        ),
      ).toBe(true);
      await settleEditor();
      expect(paths).toEqual([path, filtered, filtered]);
      expect(mounted.hasText(nextTime)).toBe(true);
      expect(mounted.hasText(previousTime)).toBe(false);
    },
    async (path) => {
      if (path === "/api/v1/admin/forum/sync-status") {
        return new Response(fixtureText("forum-sync-disabled"));
      }
      paths.push(path);
      return new Response(paths.length < 3 ? current : next);
    },
  );
});
