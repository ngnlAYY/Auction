// 改名回滚的冻结版本、在途筛选、失败与跨页回执交付。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { parseJsonObject, type Reply, TEST_SESSION_BODY } from "../api/api-request-fixture";
import { loadRenderModule } from "../solid-render-fixture.test";
import {
  byId,
  click,
  input,
  type MountedPage,
  okReply,
  type PageRequest,
  STORAGE_UNAVAILABLE,
  withPage,
} from "./browser-page-fixture.test";
import { requirePendingCommandHost } from "./pending";
import { createComponent, settle } from "./solid-dom-fixture";
import { findButton } from "./t10-view-fixture.test";

const USER = "00000000-0000-4000-8000-000000000002";
const RENAME = "00000000-0000-4000-8000-000000000001";
const LIST = "/api/v1/admin/renames";
const REVERT = `${LIST}/${RENAME}/revert`;
const SUCCESS = "改名已回滚。";
const READ_ERROR = "所需服务暂时不可用。请稍后再试。";
const HISTORY_ROW = {
  id: RENAME,
  user_id: USER,
  previous_username: "before",
  next_username: "after",
  actor: "operator",
  created_at: "2026-11-14T00:00:00Z",
  reverted_at: null,
  user_state_version: 3,
};

function history(size: number, version: number) {
  return okReply({
    pagination: { page: 1, page_size: size, total: 1 },
    renames: [{ ...HISTORY_ROW, user_state_version: version }],
  });
}

function receipt(fields: object = {}): Reply {
  return okReply({
    user: {
      id: USER,
      username: "before",
      forum_account_id: null,
      status: "active",
      key_status: "active",
      state_version: 4,
      created_at: "2026-11-14T00:00:00Z",
      updated_at: "2026-11-14T00:00:00Z",
      ...fields,
    },
  });
}

function reject(status: number, code: string): Reply {
  return { status, body: JSON.stringify({ code, message: "拒绝", data: null, request_id: null }) };
}

const notices = (view: MountedPage, value: string) =>
  [...view.document.querySelectorAll("p"), ...view.document.querySelectorAll("h2")].filter(
    (node) => node.textContent === value,
  );
const paths = (requests: readonly PageRequest[]) =>
  requests.map(({ method, url }) => [
    method,
    url.pathname,
    url.searchParams.get("user_id"),
    url.searchParams.get("name"),
  ]);

async function withRenames(
  respond: (request: PageRequest) => Reply | Promise<Reply>,
  check: (view: MountedPage, leave: () => Promise<void>) => Promise<void>,
) {
  const { page } = await loadRenderModule<Pick<typeof import("./renames"), "page">>(
    `${import.meta.dir}/renames.tsx`,
    "page",
  );
  const { PendingCommandNotices } = await loadRenderModule<
    Pick<typeof import("./pending/panel"), "PendingCommandNotices">
  >(`${import.meta.dir}/pending/panel.tsx`, "PendingCommandNotices");
  const shown = createCell(true);
  await withPage(
    "/renames?page_size=10",
    () => {
      const host = requirePendingCommandHost();
      return [
        createComponent(PendingCommandNotices, { host }),
        createComponent<{ readonly when: boolean; readonly children: JSX.Element }>(Show, {
          get when() {
            return shown();
          },
          get children() {
            return createComponent(page, {});
          },
        }),
      ];
    },
    (request) => {
      if (request.url.pathname === "/api/v1/admin/session")
        return { body: JSON.stringify(TEST_SESSION_BODY) };
      if (request.url.pathname === "/api/v1/browser-errors") return okReply({});
      return respond(request);
    },
    (view) =>
      check(view, async () => {
        shown.set(false);
        await settle();
      }),
  );
}

async function confirm(view: MountedPage) {
  await click(view.document, "回滚");
  await click(view.document, "确认");
}

test("回滚确认在筛选及重读均失败时仍只显示一次，离页则由会话交付一次", async () => {
  for (const leaveEarly of [false, true]) {
    const write = Promise.withResolvers<Reply>();
    let visibleReads = 0;
    await withRenames(
      ({ method, url }) => {
        if (method === "POST" && url.pathname === REVERT) return write.promise;
        expect(method).toBe("GET");
        expect(url.pathname).toBe(LIST);
        if (url.searchParams.has("user_id")) return history(10, 3);
        visibleReads += 1;
        return visibleReads === 1 ? history(10, 3) : STORAGE_UNAVAILABLE;
      },
      async (view, leave) => {
        await confirm(view);
        await input(byId(view.document, "rename-name"), "changed");
        expect(notices(view, READ_ERROR)).toHaveLength(1);
        expect(notices(view, SUCCESS)).toHaveLength(0);
        if (leaveEarly) await leave();
        write.resolve(receipt());
        await settle();
        if (leaveEarly) {
          expect(notices(view, SUCCESS)).toHaveLength(0);
          const shell = `回滚（user:${USER}）已成功。`;
          expect(
            notices(view, shell).filter((node) =>
              node.parentElement?.matches(".admin-command-notices__receipt"),
            ),
          ).toHaveLength(1);
          expect(
            notices(view, shell).filter(
              (node) => node.parentElement?.getAttribute("role") === "status",
            ),
          ).toHaveLength(1);
          await click(view.document, "关闭回执");
          expect(notices(view, shell)).toHaveLength(0);
        } else {
          expect(notices(view, SUCCESS)).toHaveLength(1);
          expect(notices(view, READ_ERROR)).toHaveLength(1);
          expect(notices(view, `回滚（user:${USER}）已成功。`)).toHaveLength(0);
        }
        expect(
          paths(view.requests.filter(({ url }) => url.pathname !== "/api/v1/admin/session")),
        ).toEqual([
          ["GET", LIST, null, null],
          ["GET", LIST, USER, null],
          ["POST", REVERT, null, null],
          ["GET", LIST, null, "changed"],
          ...(leaveEarly ? [] : [["GET", LIST, null, "changed"]]),
        ]);
      },
    );
  }
});

test("回滚错配保持未知，重试复用首次历史版本与原请求；写入 state_conflict 不结清", async () => {
  const responses = [
    receipt({ username: "after" }),
    receipt({ id: "00000000-0000-4000-8000-000000000004" }),
    receipt({ state_version: 5 }),
    reject(409, "state_conflict"),
    receipt(),
  ];
  await withRenames(
    ({ method, url }) => {
      if (method === "GET") return history(10, url.searchParams.has("user_id") ? 3 : 9);
      const next = responses.shift();
      if (next === undefined) throw new Error("额外的回滚请求");
      return next;
    },
    async (view) => {
      await confirm(view);
      for (let index = 0; index < 4; index += 1) {
        expect(notices(view, SUCCESS), JSON.stringify(paths(view.requests))).toHaveLength(0);
        await click(view.document, "原样重试");
      }
      expect(findButton(view.document, "原样重试")).toBeNull();
      expect(notices(view, SUCCESS)).toHaveLength(1);
      expect(view.requests.filter(({ url }) => url.searchParams.has("user_id"))).toHaveLength(1);
      const writes = view.requests.filter(
        ({ method, url }) => method === "POST" && url.pathname === REVERT,
      );
      expect(paths(writes)).toEqual(Array.from({ length: 5 }, () => ["POST", REVERT, null, null]));
      const first = writes[0];
      if (first === undefined) throw new Error("未发送回滚");
      expect(parseJsonObject(first.body, "回滚")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        expected_state_version: 3,
      });
      expect(writes.map(({ body }) => body)).toEqual(Array.from({ length: 5 }, () => first.body));
    },
  );
});

test("改名历史 GET 的 409 只表示未发送；明确写拒绝等列表恢复才显示", async () => {
  for (const failBeforeWrite of [true, false]) {
    let visibleReads = 0;
    await withRenames(
      ({ method, url }) => {
        if (url.searchParams.has("user_id")) {
          return failBeforeWrite ? reject(409, "rename_not_revertable") : history(10, 3);
        }
        if (method === "POST") return reject(409, "rename_not_revertable");
        visibleReads += 1;
        return visibleReads === 2 ? STORAGE_UNAVAILABLE : history(10, 3);
      },
      async (view) => {
        await confirm(view);
        expect(
          view.requests.filter(({ method, url }) => method === "POST" && url.pathname === REVERT),
        ).toHaveLength(failBeforeWrite ? 0 : 1);
        expect(findButton(view.document, "原样重试")).toBeNull();
        expect(notices(view, SUCCESS)).toHaveLength(0);
        expect(notices(view, READ_ERROR)).toHaveLength(1);
        const errors = () =>
          byId(view.document, "admin-renames-title")
            .parentElement?.querySelectorAll("p")
            .filter(
              (node) =>
                node.getAttribute("role") === "alert" && node.matches(".action-feedback--error"),
            ) ?? [];
        expect(errors()).toHaveLength(0);
        await input(byId(view.document, "rename-name"), "recover");
        expect(errors().map(({ textContent }) => textContent)).toEqual([
          "操作失败。请重新加载当前数据后再试。",
        ]);
      },
    );
  }
});

test("回滚 POST 503 后原样重试，可信的 POST 409 了结先前的未知记录", async () => {
  let historyReads = 0;
  let writes = 0;
  await withRenames(
    ({ method, url }) => {
      if (method === "POST" && url.pathname === REVERT) {
        writes += 1;
        return writes === 1 ? STORAGE_UNAVAILABLE : reject(409, "rename_not_revertable");
      }
      expect(method).toBe("GET");
      expect(url.pathname).toBe(LIST);
      if (url.searchParams.has("user_id")) {
        historyReads += 1;
        return history(10, historyReads === 1 ? 3 : 9);
      }
      return history(10, 9);
    },
    async (view) => {
      await confirm(view);
      expect(findButton(view.document, "原样重试")).not.toBeNull();
      expect(notices(view, SUCCESS)).toHaveLength(0);

      await click(view.document, "原样重试");
      expect(findButton(view.document, "原样重试")).toBeNull();
      expect(notices(view, SUCCESS)).toHaveLength(0);
      expect(findButton(view.document, "回滚")?.hasAttribute("disabled")).toBe(false);
      expect(paths(view.requests.filter(({ url }) => url.searchParams.has("user_id")))).toEqual([
        ["GET", LIST, USER, null],
      ]);
      const posts = view.requests.filter(
        ({ method, url }) => method === "POST" && url.pathname === REVERT,
      );
      expect(posts).toHaveLength(2);
      const [first, retry] = posts;
      if (first === undefined || retry === undefined) throw new Error("缺少回滚初次请求或重试");
      expect(parseJsonObject(first.body, "回滚")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        expected_state_version: 3,
      });
      expect(retry.body).toBe(first.body);
    },
  );
});
