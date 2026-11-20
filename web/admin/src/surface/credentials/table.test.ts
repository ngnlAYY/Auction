// 凭据操作与版本读取、跨行卸载回执、改名验证的联动。

import { expect, mock, spyOn, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import {
  createAdminRequestHarness,
  parseJsonObject,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import type { AdminCredential, AdminCredentialPage } from "../../api/credentials";
import { adminApiFailureFromHttpBody } from "../../api/failure";
import { mustSucceed } from "../../api/t10-contract-fixture.test";
import { adminLocaleSession } from "../admin-locale-fixture.test";
import type { TestDocument } from "../solid-dom-fixture";
import { createComponent, settle } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  field,
  findButton,
  hasText,
  loadCredentialTable,
  onlyElement,
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
    await withT10View(
      () =>
        createComponent(LocaleSessionContext, {
          value: adminLocaleSession,
          get children() {
            return Fixture(row === undefined ? { host } : { host, row });
          },
        }),
      run,
    );
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

test("credential_pagination_pushes_navigation_and_replaces_only_the_invalid_page", async () => {
  const Fixture = await loadCredentialTable();
  const credentials = await import("../../api/credentials");
  const first = "/cookies?page_size=10";
  const second = "/cookies?page_size=10&page=2";
  const { fake, restore } = installFakeWindow("/audit");
  fake.history.pushState(null, "", first);
  const harness = createAdminRequestHarness();
  const read = spyOn(credentials, "fetchAdminCredentials");
  const initial = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const failed = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const empty = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const corrected = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const projection = (page: number, total: number): AdminCredentialPage => ({
    credentials: [],
    page,
    page_size: 10,
    total,
    restart_required: true,
  });
  try {
    read.mockImplementationOnce(() => initial.promise);
    read.mockImplementationOnce(() => failed.promise);
    read.mockImplementationOnce(() => empty.promise);
    read.mockImplementationOnce(() => corrected.promise);
    const { host } = harness.registerHost();
    await withT10View(
      () => Fixture({ host }),
      async (document) => {
        const writes = () =>
          fake.writes.splice(0).map(({ mode, url }) => [mode, url.pathname + url.search]);
        writes();
        initial.resolve(ok(projection(1, 11)));
        await settle();
        expect(writes()).toEqual([]);

        onlyElement(document, '[aria-label="下一页"]').click();
        await settle();
        expect(writes()).toEqual([["push", second]]);
        failed.resolve(err("storage_unavailable"));
        await settle();
        expect(writes()).toEqual([]);
        expect(fake.location.pathname + fake.location.search).toBe(second);

        await clickButton(document, "重试");
        await settle();
        empty.resolve(ok(projection(2, 10)));
        await settle();
        expect(writes()).toEqual([["replace", first]]);
        corrected.resolve(ok(projection(1, 10)));
        await settle();
        expect(writes()).toEqual([]);
        expect(read.mock.calls.map(([, , , , page, size]) => [page, size])).toEqual([
          [1, 10],
          [2, 10],
          [2, 10],
          [1, 10],
        ]);
        fake.history.back();
        expect(fake.location.pathname + fake.location.search).toBe(first);
        fake.history.back();
        expect(fake.location.pathname).toBe("/audit");
      },
    );
  } finally {
    read.mockRestore();
    harness.cleanup();
    restore();
  }
});

test("credential_actions_keep_their_own_payload_while_an_import_is_unknown", async () => {
  const credentials = await import("../../api/credentials");
  const harness = createAdminRequestHarness();
  const read = spyOn(credentials, "fetchAdminCredentials");
  const fresh = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  const empty = Promise.withResolvers<Result<AdminCredentialPage, string>>();
  read.mockResolvedValueOnce(ok(projection(CREDENTIAL)));
  read.mockImplementationOnce(() => fresh.promise);
  read.mockImplementationOnce(() => empty.promise);
  const session = { body: JSON.stringify(TEST_SESSION_BODY) };
  const endpoint = "/api/v1/admin/credentials";
  const checkPath = `${endpoint}/${CREDENTIAL.id}/check`;
  function response(data: object) {
    return { body: JSON.stringify({ code: "ok", message: "", data }) };
  }
  try {
    harness.installBrowserRequest();
    const writes = harness.installFetch([
      session,
      { status: 503, body: FAILURE },
      session,
      response({
        credential_id: CREDENTIAL.id,
        state_version: 1,
        checked_at: CREDENTIAL.updated_at,
        checks: [{ world: "main", status: "inconclusive" }],
      }),
      session,
      { body: mutationReply(UPDATED) },
      session,
      response({ credential_id: CREDENTIAL.id, state_version: 3, restart_required: true }),
    ]);
    await withCredentials(undefined, async (document) => {
      field(document, "import-label").input("新的凭据");
      field(document, "import-secret").input("fake-test-cookie=second-placeholder");
      submitForm(onlyElement(document, ".admin-credentials-import"));
      await settle();
      expect(document.getElementById("import-unknown-help")).not.toBeNull();

      await clickButton(document, "校验 Cookie");
      await clickButton(document, "停用");
      fresh.resolve(ok(projection(UPDATED)));
      await settle();
      await clickButton(document, "删除");
      const dialog = onlyElement(document, '[role="dialog"]');
      const confirm = dialog.querySelector(".admin-btn-danger");
      if (confirm === null) throw new Error("缺少删除确认按钮");
      confirm.click();
      await settle();
      empty.resolve(ok({ ...projection(UPDATED), credentials: [], total: 0 }));
      await settle();

      const business = writes.filter(({ method }) => method !== "GET");
      expect(business.map(({ method, url }) => [method, url])).toEqual([
        ["POST", endpoint],
        ["POST", checkPath],
        ["PATCH", endpoint],
        ["DELETE", endpoint],
      ]);
      const check = parseJsonObject(business[1]?.body ?? "", "校验正文");
      const patch = parseJsonObject(business[2]?.body ?? "", "停用正文");
      const deletion = parseJsonObject(business[3]?.body ?? "", "删除正文");
      // 比较字段集合再比较安全值，意外包含的秘密不会出现在断言的实际值里。
      expect(Object.keys(check).sort()).toEqual(["expected_state_version"]);
      expect(check.expected_state_version).toBe(1);
      expect(Object.keys(patch).sort()).toEqual([
        "action",
        "command_id",
        "credential_id",
        "expected_state_version",
        "label",
        "status",
      ]);
      expect(patch.credential_id === CREDENTIAL.id && patch.expected_state_version === 1).toBe(
        true,
      );
      expect(
        patch.action === "set_status" && patch.status === "disabled" && patch.label === null,
      ).toBe(true);
      expect(Object.keys(deletion).sort()).toEqual([
        "command_id",
        "credential_id",
        "expected_state_version",
      ]);
      expect(
        deletion.credential_id === CREDENTIAL.id && deletion.expected_state_version === 2,
      ).toBe(true);
      const importId = parseJsonObject(business[0]?.body ?? "", "导入正文").command_id;
      expect(new Set([importId, patch.command_id, deletion.command_id]).size).toBe(3);
      expect(document.getElementById("import-unknown-help")).not.toBeNull();
      expect(findButton(document, "原样重试")).not.toBeNull();
    });
  } finally {
    read.mockRestore();
    harness.cleanup();
  }
});
