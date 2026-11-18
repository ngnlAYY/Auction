// 命令宿主的身份判定与新提交条件。

import { expect, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";

import type { PublicSessionResult } from "..";
import { catalogCommandSession } from "./command-scope";

const USER_ID = "018f4c8e-7b2a-7c3d-8e4f-000000000001";
const OTHER_USER_ID = "018f4c8e-7b2a-7c3d-8e4f-000000000002";

function authenticated(userId: string): PublicSessionResult {
  return ok({
    authenticated: true,
    user_id: userId,
    username: "viewer",
    expires_at: null,
    absolute_expires_at: null,
  });
}

test("同身份复核时保留命令表单，复核成功后恢复新提交", () => {
  const snapshot = authenticated(USER_ID);
  expect(catalogCommandSession(snapshot, false, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: false,
  });
  expect(catalogCommandSession(snapshot, true, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: true,
  });
});

test("暂时读取失败仍保留已确认身份的草稿，暂停新提交", () => {
  expect(catalogCommandSession(err("network_error"), true, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: false,
  });
});

test("匿名、不同身份、从未确认的会话都不能启用表单或提交", () => {
  for (const [snapshot, confirmed] of [
    [ok(null), USER_ID],
    [authenticated(OTHER_USER_ID), USER_ID],
    [undefined, null],
    [authenticated(USER_ID), null],
  ] as const) {
    expect(catalogCommandSession(snapshot, true, USER_ID, confirmed)).toEqual({
      authenticated: false,
      canSubmit: false,
    });
  }
});
