// 用户行通过真实写入与未决命令宿主交付重签 Key。

import { expect, test } from "bun:test";
import { parseJsonObject, TEST_SESSION_BODY } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { click, okReply, withPage } from "../browser-page-fixture.test";
import { createComponent, settle } from "../solid-dom-fixture";
import { findButton } from "../t10-view-fixture.test";

const USERS = "/api/v1/admin/users";
const KEY = "A".repeat(43);
const OTHER_KEY = "B".repeat(43);
const USER = {
  id: "00000000-0000-4000-8000-000000000002",
  username: "玩家",
  forum_account_id: null,
  status: "active",
  key_status: "active",
  state_version: 3,
  created_at: "2026-11-14T00:00:00Z",
  updated_at: "2026-11-14T00:00:00Z",
};

test("用户行重签 Key 错目标保持未决，原样重试后只交付正确 Key 一次", async () => {
  const { page } = await loadRenderModule<Pick<typeof import("."), "page">>(
    `${import.meta.dir}/index.tsx`,
    "page",
  );
  let writes = 0;
  await withPage(
    "/users?page_size=10",
    () => createComponent(page, {}),
    ({ method, url }) => {
      if (url.pathname === "/api/v1/admin/session")
        return { body: JSON.stringify(TEST_SESSION_BODY) };
      if (url.pathname === "/api/v1/browser-errors") return okReply({});
      expect(url.pathname).toBe(USERS);
      if (method === "GET") {
        return okReply({
          pagination: { page: 1, page_size: 10, total: 1 },
          users: [
            {
              ...USER,
              blacklist_count: 0,
              blacklist_history: [],
              state_version: writes === 2 ? 4 : 3,
            },
          ],
        });
      }
      expect(method).toBe("PATCH");
      writes += 1;
      const wrongTarget = writes === 1;
      return okReply({
        user: {
          ...USER,
          id: wrongTarget ? "00000000-0000-4000-8000-000000000004" : USER.id,
          state_version: 4,
        },
        generated_key: wrongTarget ? OTHER_KEY : KEY,
      });
    },
    async ({ document, requests }) => {
      const keys = () =>
        document
          .querySelectorAll(".admin-users-secret")
          .flatMap((dialog) =>
            dialog.querySelectorAll("code").map(({ textContent }) => textContent),
          );
      await click(document, "重新签发 Key");
      await click(document, "确认");
      expect(keys()).toEqual([]);
      expect(findButton(document, "原样重试")).not.toBeNull();

      await click(document, "原样重试");
      expect(keys()).toEqual([KEY]);
      expect(findButton(document, "原样重试")).toBeNull();
      const patches = requests.filter(
        ({ method, url }) => method === "PATCH" && url.pathname === USERS,
      );
      expect(patches).toHaveLength(2);
      const [first, retry] = patches;
      if (first === undefined || retry === undefined) throw new Error("缺少重签请求或重试");
      expect(parseJsonObject(first.body, "重签 Key")).toEqual({
        action: "reissue_key",
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        expected_state_version: 3,
        forum_account_id: null,
        user_id: USER.id,
        username: null,
      });
      expect(retry.body).toBe(first.body);

      await click(document, "关闭");
      await settle();
      expect(keys()).toEqual([]);
      expect(findButton(document, "关闭")).toBeNull();
      expect(writes).toBe(2);
    },
  );
});
