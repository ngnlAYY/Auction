// 凭据操作与版本读取、跨行卸载回执、改名验证的联动。

import { expect, mock, spyOn, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import {
  createAdminRequestHarness,
  parseJsonObject,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import type { AdminCredential, AdminCredentialPage } from "../../api/credentials";
import { adminApiFailureFromHttpBody } from "../../api/failure";
import { mustSucceed } from "../../api/t10-contract-fixture.test";
import type { TestDocument } from "../solid-dom-fixture";
import { settle } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  field,
  findButton,
  hasText,
  loadCredentialTable,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";
import type { CredentialTableFixtureProps } from "./table-fixture";

const CREDENTIAL: AdminCredential = {
  id: "0f8fad5b-d9cb-469f-a165-70867728950e",
  kind: "mail_cookie",
  world: "main",
  label: "测试凭据",
  player_name: null,
  status: "active",
  state_version: 1,
  created_at: "2026-11-18T00:00:00Z",
  rotated_at: null,
  updated_at: "2026-11-18T00:00:00Z",
};
const UPDATED = { ...CREDENTIAL, status: "disabled", state_version: 2 };
const RESTART = "操作已完成。请重启 Worker 以应用凭据变更。";
const FAILURE = '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}';
const SESSION_PATH = "/api/v1/admin/session?include=server_time";

function projection(credential: AdminCredential): AdminCredentialPage {
  return { credentials: [credential], page: 1, page_size: 10, total: 1, restart_required: false };
}

function mutationReply(credential: AdminCredential): string {
  return JSON.stringify({ code: "ok", message: "", data: { credential, restart_required: true } });
}

async function withCredentials(
  row: CredentialTableFixtureProps["row"],
  run: (document: TestDocument) => Promise<void>,
) {
  const Fixture = await loadCredentialTable();
  const harness = createAdminRequestHarness();
  let restoreWindow: (() => void) | undefined;
  try {
    restoreWindow = installFakeWindow("/credentials?page_size=10").restore;
    const { host } = harness.registerHost();
    await withT10View(() => Fixture(row === undefined ? { host } : { host, row }), run);
  } finally {
    try {
      harness.cleanup();
    } finally {
      restoreWindow?.();
    }
  }
}

test("credential_restart_receipt_survives_the_failed_list_read", async () => {
  const credentials = await import("../../api/credentials");
  const initial = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const failed = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const refreshed = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const deliverWriteReceipt = Promise.withResolvers<void>();
  const requests = createAdminRequestHarness();
  let restoreRead: (() => void) | undefined;
  let restoreMutation: (() => void) | undefined;
  try {
    const read = spyOn(credentials, "fetchAdminCredentials");
    restoreRead = () => read.mockRestore();
    const mutation = spyOn(credentials, "mutateAdminCredential");
    restoreMutation = () => mutation.mockRestore();
    read.mockImplementationOnce(() => initial.promise);
    read.mockImplementationOnce(() => failed.promise);
    read.mockImplementationOnce(() => refreshed.promise);
    requests.installBrowserRequest();
    const writes = requests.installFetch([
      { body: JSON.stringify(TEST_SESSION_BODY) },
      { body: mutationReply(UPDATED) },
    ]);
    // 先记录真实请求，等用例显式交付 PATCH 回执后才让写入继续。
    const reply = globalThis.fetch;
    globalThis.fetch = Object.assign(
      async (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
        const response = await reply(input, init);
        if (init?.method === "PATCH") await deliverWriteReceipt.promise;
        return response;
      },
      { preconnect: reply.preconnect },
    );
    await withCredentials(undefined, async (document) => {
      expect(read).toHaveBeenCalledTimes(1);
      initial.resolve(ok(projection(CREDENTIAL)));
      await settle();
      await clickButton(document, "停用");
      expect(mutation).toHaveBeenCalledTimes(1);
      expect(writes.map(({ method, url }) => [method, url])).toEqual([
        ["GET", SESSION_PATH],
        ["PATCH", "/api/v1/admin/credentials"],
      ]);
      const result = mutation.mock.results[0];
      if (result?.type !== "return") throw new Error("凭据写入未启动");
      expect(hasText(document, RESTART)).toBe(false);
      expect(read).toHaveBeenCalledTimes(1);

      deliverWriteReceipt.resolve();
      expect(mustSucceed(await result.value).restart_required).toBe(true);
      await settle();
      expect(hasText(document, RESTART)).toBe(true);
      expect(read).toHaveBeenCalledTimes(2);

      failed.resolve(err(adminApiFailureFromHttpBody(503, FAILURE).code));
      await settle();
      expect(findButton(document, "停用")).toBeNull();
      expect(hasText(document, "所需服务暂时不可用。请稍后再试。")).toBe(true);
      expect(hasText(document, RESTART)).toBe(true);
      expect(findButton(document, "关闭回执")).not.toBeNull();

      await clickButton(document, "重试");
      expect(read).toHaveBeenCalledTimes(3);
      expect(mutation).toHaveBeenCalledTimes(1);
      expect(writes.filter(({ method }) => method === "PATCH")).toHaveLength(1);
      expect(
        read.mock.calls.map(([, kind, world, status, page, size]) => [
          kind,
          world,
          status,
          page,
          size,
        ]),
      ).toEqual([
        ["", "", "", 1, 10],
        ["", "", "", 1, 10],
        ["", "", "", 1, 10],
      ]);
      refreshed.resolve(ok(projection(UPDATED)));
      await settle();
      expect(findButton(document, "启用")).not.toBeNull();
      expect(hasText(document, RESTART)).toBe(true);
    });
  } finally {
    deliverWriteReceipt.resolve();
    try {
      restoreMutation?.();
    } finally {
      try {
        restoreRead?.();
      } finally {
        requests.cleanup();
      }
    }
  }
});

test("credential_actions_wait_for_the_new_version_while_the_old_row_stays_visible", async () => {
  const credentials = await import("../../api/credentials");
  const updated = Promise.withResolvers<Result<AdminCredential, string>>();
  const writes = createAdminRequestHarness();
  let restoreMutation: (() => void) | undefined;
  try {
    const mutation = spyOn(credentials, "mutateAdminCredential");
    restoreMutation = () => mutation.mockRestore();
    const reply =
      Promise.withResolvers<Awaited<ReturnType<typeof credentials.mutateAdminCredential>>>();
    mutation.mockImplementation(() => reply.promise);
    const readVersion = mock(() => updated.promise);
    const requests = writes.installFetch([]);
    await withCredentials({ initial: CREDENTIAL, read: readVersion }, async (document) => {
      expect(button(document, "停用").hasAttribute("disabled")).toBe(false);
      await clickButton(document, "使凭据投影失效");
      expect(readVersion).toHaveBeenCalledTimes(1);
      const stale = button(document, "停用");
      expect(stale.hasAttribute("disabled")).toBe(true);
      expect(typeof stale.attributes.onClick).toBe("function");
      stale.dispatchClick();
      await settle();
      expect(hasText(document, "已经发送凭据写入")).toBe(false);
      expect(mutation).toHaveBeenCalledTimes(0);
      expect(requests).toHaveLength(0);

      updated.resolve(ok(UPDATED));
      await settle();
      await clickButton(document, "启用");
      expect(hasText(document, "已经发送凭据写入")).toBe(true);
      expect(mutation).toHaveBeenCalledTimes(1);
      const prepared = mutation.mock.calls[0]?.[1];
      if (prepared === undefined) throw new Error("缺少启用命令");
      expect(mustSucceed(prepared).previous.state_version).toBe(2);
    });
  } finally {
    try {
      restoreMutation?.();
    } finally {
      writes.cleanup();
    }
  }
});

test("a_removed_credential_row_still_delivers_the_restart_warning", async () => {
  const credentials = await import("../../api/credentials");
  for (const unloadRow of [false, true]) {
    const reply =
      Promise.withResolvers<Awaited<ReturnType<typeof credentials.mutateAdminCredential>>>();
    let restoreMutation: (() => void) | undefined;
    try {
      const mutation = spyOn(credentials, "mutateAdminCredential");
      restoreMutation = () => mutation.mockRestore();
      mutation.mockImplementation(() => reply.promise);
      await withCredentials(
        {
          initial: CREDENTIAL,
          read: async () => ok(UPDATED),
        },
        async (document) => {
          await clickButton(document, "停用");
          expect(mutation).toHaveBeenCalledTimes(1);
          expect(hasText(document, RESTART)).toBe(false);
          if (unloadRow) {
            await clickButton(document, "凭据翻页");
            expect(findButton(document, "停用")).toBeNull();
            expect(hasText(document, RESTART)).toBe(false);
          }

          reply.resolve(ok({ credential: UPDATED, restart_required: true }));
          await settle();
          expect(hasText(document, RESTART)).toBe(true);
          expect(findButton(document, "关闭回执")).not.toBeNull();
        },
      );
    } finally {
      restoreMutation?.();
    }
  }
});

test("rename_rejects_invalid_labels_before_submitting_and_accepts_corrections", async () => {
  const credentials = await import("../../api/credentials");
  const requests = createAdminRequestHarness();
  const renamed = { ...CREDENTIAL, label: "新标签", state_version: 2 };
  const submissions: (readonly [entity: string, action: string])[] = [];
  let restoreMutation: (() => void) | undefined;
  try {
    const mutation = spyOn(credentials, "mutateAdminCredential");
    restoreMutation = () => mutation.mockRestore();
    requests.installBrowserRequest();
    const writes = requests.installFetch([
      { body: JSON.stringify(TEST_SESSION_BODY) },
      { body: mutationReply(renamed) },
    ]);
    await withCredentials(
      {
        initial: { ...CREDENTIAL, label: "旧标签" },
        read: async () => ok(renamed),
        onSubmitWithReceipt: (entity, action) => submissions.push([entity, action]),
      },
      async (document) => {
        await clickButton(document, "改名");
        const inputId = "label-0f8fad5b-d9cb-469f-a165-70867728950e";
        const input = field(document, inputId);
        const form = document
          .querySelectorAll("form")
          .find((node) => node.querySelectorAll("input").includes(input));
        if (form === undefined) throw new Error("找不到改名输入框所属表单");
        const errorId = "label-0f8fad5b-d9cb-469f-a165-70867728950e-error";
        expect(input.value).toBe("旧标签");

        input.input(" ");
        await settle();
        submitForm(form);
        await settle();
        expect(submissions).toEqual([]);
        expect(writes).toHaveLength(0);
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(input.getAttribute("aria-describedby")).toBe(errorId);
        expect(field(document, errorId).textContent).toBe("请填写此字段。");

        input.input("仍含空白 ");
        await settle();
        expect(input.getAttribute("aria-invalid")).toBe("true");
        submitForm(form);
        await settle();
        expect(submissions).toEqual([]);
        expect(writes).toHaveLength(0);

        input.input("新标签");
        await settle();
        expect(input.getAttribute("aria-invalid")).toBe("false");
        submitForm(form);
        await settle();
        expect(submissions).toEqual([["credential:0f8fad5b-d9cb-469f-a165-70867728950e", "改名"]]);
        const result = mutation.mock.results[0];
        if (result?.type !== "return") throw new Error("改名写入未启动");
        expect(mustSucceed(await result.value).credential.label).toBe("新标签");
        await settle();
        expect(writes.map(({ method, url }) => [method, url])).toEqual([
          ["GET", SESSION_PATH],
          ["PATCH", "/api/v1/admin/credentials"],
        ]);
        const sent = writes[1];
        if (sent === undefined) throw new Error("缺少改名请求");
        expect(parseJsonObject(sent.body, "改名请求")).toMatchObject({
          credential_id: "0f8fad5b-d9cb-469f-a165-70867728950e",
          expected_state_version: 1,
          action: "rename",
          label: "新标签",
          status: null,
        });
      },
    );
  } finally {
    try {
      restoreMutation?.();
    } finally {
      requests.cleanup();
    }
  }
});
