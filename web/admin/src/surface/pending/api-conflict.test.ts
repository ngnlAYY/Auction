// 首次 API 版本冲突经真实写端点返回，并释放来源页实体。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";
import { flush } from "solid-js";
import { installBrowserRequest, installWriteFetch } from "../../api/api-request-fixture";
import { prepareAdminRulesPublish, publishAdminRules } from "../../api/rules";
import { type AdminUser, mutateAdminUser, prepareAdminUserMutationOnce } from "../../api/users";
import { createPendingHarness, settlePending } from "./pending-command-fixture";

async function settleApi(): Promise<void> {
  for (let round = 0; round < 12; round += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    flush();
  }
  await settlePending();
}

function conflict(body: string): Response {
  return new Response(body, {
    status: 409,
    headers: { "content-type": "application/json" },
  });
}

const USER: AdminUser = {
  id: "11111111-1111-4111-8111-111111111111",
  username: "玩家甲",
  forum_account_id: null,
  status: "active",
  key_status: "active",
  state_version: 5,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  blacklist_count: 0,
  blacklist_history: [],
};

let restoreRequest = (): void => {};
beforeEach(() => {
  restoreRequest = installBrowserRequest();
});

afterEach(() => {
  restoreRequest();
});

test("首次用户 API 版本冲突释放实体", async () => {
  const harness = createPendingHarness();
  try {
    const fetch = installWriteFetch(() =>
      conflict(
        '{"code":"state_conflict","message":"用户版本已变化","data":{"current_state_version":6},"request_id":null}',
      ),
    );
    try {
      const observed: { value: string | null } = { value: null };
      let successCalls = 0;
      const prepare = prepareAdminUserMutationOnce(USER, "rename", "玩家乙", null);
      let submittedId = "";
      expect(
        harness.origin.commands.submit(
          "first",
          "test",
          async (commandId, signal) => {
            submittedId = commandId;
            return mutateAdminUser(harness.host.sessionHost, commandId, prepare, signal);
          },
          () => {
            successCalls += 1;
            throw new Error("版本冲突不能交付成功结果");
          },
          (code) => {
            observed.value = code;
          },
        ),
      ).toBe(true);
      expect(harness.origin.commands.contains("first")).toBe(true);
      await settleApi();
      expect(observed.value).toBe("state_conflict");
      expect(successCalls).toBe(0);
      expect(harness.origin.commands.contains("first")).toBe(false);

      expect(fetch.writes).toHaveLength(1);
      const request = fetch.writes[0];
      if (request === undefined) throw new Error("缺少用户写入");
      expect(request.method).toBe("PATCH");
      expect(request.url).toBe("https://auction.test/api/v1/admin/users");
      const body = parseJson(request.body);
      if (!body.ok) throw new Error("用户写入不是 JSON");
      expect(body.value).toEqual({
        command_id: submittedId,
        user_id: USER.id,
        expected_state_version: 5,
        action: "rename",
        username: "玩家乙",
        forum_account_id: null,
      });
    } finally {
      fetch.restore();
    }
  } finally {
    harness.dispose();
  }
});

test("首次规则与关于文档版本冲突释放实体", async () => {
  for (const [kind, path] of [
    ["rules", "/api/v1/admin/rules"],
    ["about", "/api/v1/admin/about"],
  ] as const) {
    const harness = createPendingHarness();
    try {
      const fetch = installWriteFetch(() =>
        conflict(
          '{"code":"state_conflict","message":"文档版本已变化","data":{"current_revision":6},"request_id":null}',
        ),
      );
      try {
        const observed: { value: string | null } = { value: null };
        let successCalls = 0;
        const prepare = prepareAdminRulesPublish(5, "zh-CN", "标题", "正文");
        let submittedId = "";
        expect(
          harness.origin.commands.submit(
            "first",
            "test",
            async (commandId, signal) => {
              submittedId = commandId;
              const result = await publishAdminRules(
                harness.host.sessionHost,
                kind,
                commandId,
                prepare,
                signal,
              );
              return result.ok ? result : err(result.error.code);
            },
            () => {
              successCalls += 1;
              throw new Error("版本冲突不能交付成功结果");
            },
            (code) => {
              observed.value = code;
            },
          ),
        ).toBe(true);
        expect(harness.origin.commands.contains("first")).toBe(true);
        await settleApi();
        expect(observed.value).toBe("state_conflict");
        expect(successCalls).toBe(0);
        expect(harness.origin.commands.contains("first")).toBe(false);

        expect(fetch.writes).toHaveLength(1);
        const request = fetch.writes[0];
        if (request === undefined) throw new Error("缺少文档写入");
        expect(request.method).toBe("PUT");
        expect(request.url).toBe(`https://auction.test${path}`);
        const body = parseJson(request.body);
        if (!body.ok) throw new Error("文档写入不是 JSON");
        expect(body.value).toEqual({
          command_id: submittedId,
          expected_revision: 5,
          locale: "zh-CN",
          title: "标题",
          markdown: "正文",
        });
      } finally {
        fetch.restore();
      }
    } finally {
      harness.dispose();
    }
  }
});
