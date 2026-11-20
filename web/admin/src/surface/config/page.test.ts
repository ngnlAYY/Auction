// 运行配置页的读取围栏、草稿导航、写入与 Worker 回执。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { createRoot, flush } from "solid-js";
import type { CapturedWrite } from "../../api/api-request-fixture";
import { cleanupScope } from "../../test-scope-fixture";
import { captureEvent, findElement, required, type TestElement } from "../solid-dom-fixture";
import { configDocumentPage, refreshConfigDraft } from "./config-document-fixture.test";
import {
  button,
  clickButton,
  deferredReply,
  maybeButton,
  noticeLine,
  settle,
} from "./config-page-fixture.test";
import {
  accepted,
  apiFailure,
  configReadReply,
  decodeConfigData,
  defaultProjection,
  documentValue,
  jsonObject,
  objectAt,
  valueAt,
} from "./config-projection-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";
import { type ConfigDraft, useConfigDraft } from "./draft";
import {
  runtimeConfigFormFromResponse,
  runtimeConfigFormSetInput,
  runtimeConfigFormSignature,
  runtimeConfigFormSourceDocument,
} from "./model";
import { configChangeNeedsRestart, configChanges } from "./review";

const PATH = "operations.worker.poll_interval_seconds";
const LEASE = "operations.forum.lease_duration_seconds";
const UNAVAILABLE = "所需服务暂时不可用。请稍后再试。";
const LOADING = "正在加载运行时配置……";
const CONFLICT = "配置已经变化，请重新加载后再次校验。";
const WORKER_UNKNOWN =
  "无法确认运行中的 Worker 是否已采用当前配置（没有可比较的在线 Worker，或其版本不提供这项信息）。";
const WORKER_CONSISTENT = "运行中的 Worker 已采用当前配置文件里需要重启才生效的设置。";
const WORKER_RESTART = "当前配置文件里有尚未被运行中的 Worker 采用的设置，重启 Worker 后生效。";

function fingerprint(digit: string): string {
  return `sha256:${digit.repeat(64)}`;
}

function workerProjection() {
  const result = defaultProjection();
  objectAt(result.data.document, "operations.worker").managed = true;
  return result;
}

function forumProjection() {
  const result = defaultProjection();
  const forum = objectAt(result.data.document, "operations.forum");
  forum.managed = true;
  forum.lease_duration_seconds = 300;
  forum.item_timeout_seconds = 240;
  return result;
}

function savedReply(
  snapshot: ReturnType<typeof defaultProjection>,
  request: CapturedWrite,
): Response {
  const body = jsonObject(accepted(parseJson(request.body)));
  expect(body.command_id).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu,
  );
  expect(body.command_id).not.toBe("00000000-0000-0000-0000-000000000000");
  expect(body.expected_fingerprint).toBe(snapshot.data.fingerprint);
  expect(valueAt(body.document, LEASE)).toBe(600);
  snapshot.data.document = body.document;
  snapshot.data.fingerprint = fingerprint("1");
  const receipt: Record<string, unknown> = { ...snapshot.data, restart_required: true };
  delete receipt.worker_restart_state;
  return Response.json({ ...snapshot, data: receipt });
}

function inputAt(tree: TestElement, path: string): TestElement {
  return required(
    tree
      .querySelectorAll("[data-runtime-number]")
      .find((input) => input.getAttribute("data-runtime-number") === path),
    `数字字段 ${path}`,
  );
}

function formOf(root: TestElement): TestElement {
  return required(root.querySelector("form"), "配置表单");
}

function statusRegion(tree: TestElement, message: string, warning: boolean): TestElement {
  const line = required(
    findElement(
      tree,
      (node) =>
        node.tag === "p" &&
        node.textContent === message &&
        node.parentElement?.getAttribute("role") === "status",
    ),
    `Worker 状态「${message}」`,
  );
  expect(line.matches(".admin-panel__note")).toBe(true);
  expect(line.matches(".admin-runtime-config__restart")).toBe(warning);
  return required(line.parentElement, "Worker 状态容器");
}

function expectDialogStatus(tree: TestElement, message: string): void {
  const dialog = required(
    findElement(tree, (node) => node.getAttribute("role") === "dialog"),
    "确认框",
  );
  expect(
    findElement(
      dialog,
      (node) => node.getAttribute("role") === "status" && node.textContent === message,
    ),
  ).not.toBeNull();
}

function setChecked(draft: ConfigDraft): string {
  const signature = accepted(runtimeConfigFormSignature(draft.form.peek()));
  draft.checkedSignature.set(signature);
  flush();
  return signature;
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("keyed_config_editor_reports_a_new_snapshot_historical_number_immediately", async () => {
  const snapshot = workerProjection();
  const page = configDocumentPage(() => configReadReply(snapshot), "重读测试快照");
  try {
    page.mount();
    await settle();
    const input = inputAt(page.root, PATH);
    input.input("0");
    flush();
    input.blur();
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("true");
    input.input("60");
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("false");

    objectAt(snapshot.data.document, "operations.worker").poll_interval_seconds = 0;
    snapshot.data.fingerprint = fingerprint("2");
    clickButton(page.root, "重读测试快照");
    await settle();
    const rebuilt = inputAt(page.root, PATH);
    expect(rebuilt).not.toBe(input);
    expect(rebuilt.value).toBe("0");
    expect(rebuilt.getAttribute("aria-invalid")).toBe("true");
    const description = required(rebuilt.getAttribute("aria-describedby"), "字段错误关联");
    expect(page.document.getElementById(description)).not.toBeNull();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "GET"]);
  } finally {
    page.dispose();
  }
});

test("background_reads_keep_the_edited_field_and_its_error", async () => {
  const snapshot = workerProjection();
  const same = deferredReply();
  const changed = deferredReply();
  const reads = [() => configReadReply(snapshot), same.wait, changed.wait];
  const page = configDocumentPage(
    (_request, _init, index) => required(reads[index], "读取回执")(),
    "后台重读",
  );
  try {
    page.mount();
    await settle();
    const initial = inputAt(page.root, PATH);
    const region = statusRegion(page.root, WORKER_UNKNOWN, false);
    const statusMutations = page.document.watchMounts(region);
    const fieldMutations = page.document.watchMounts(initial);
    initial.input("0");
    flush();
    initial.blur();
    flush();
    const description = required(initial.getAttribute("aria-describedby"), "字段错误关联");
    const expectFieldKept = () => {
      expect(inputAt(page.root, PATH)).toBe(initial);
      expect(initial.value).toBe("0");
      expect(initial.hasAttribute("disabled")).toBe(false);
      expect(initial.hasAttribute("readonly")).toBe(false);
      expect(initial.getAttribute("aria-invalid")).toBe("true");
      expect(initial.getAttribute("aria-describedby")).toBe(description);
      expect(fieldMutations).toEqual([]);
    };
    expectFieldKept();
    clickButton(page.root, "后台重读");
    await settle();
    expect(page.requests).toHaveLength(2);
    expect(noticeLine(page.root, "status", ".ui-async-state__title", LOADING)).not.toBeNull();
    expect(statusRegion(page.root, WORKER_UNKNOWN, false)).toBe(region);
    expect(statusMutations).toEqual([]);
    expectFieldKept();
    expect(button(page.root, "校验候选配置").hasAttribute("disabled")).toBe(true);

    jsonObject(snapshot.data.equipment_taxonomy).revision = "test-2";
    snapshot.data.worker_restart_state = "consistent";
    same.respond(configReadReply(snapshot));
    await settle();
    expect(page.root.textContent).not.toContain(WORKER_UNKNOWN);
    expect(statusRegion(page.root, WORKER_CONSISTENT, false)).toBe(region);
    expect(statusMutations).toEqual([]);
    expectFieldKept();
    expect(button(page.root, "校验候选配置").hasAttribute("disabled")).toBe(false);
    expect(page.draft.form.peek().equipmentTaxonomy?.revision).toBe("test-2");
    expect(page.draft.dirty()).toBe(true);

    snapshot.data.fingerprint = fingerprint("2");
    clickButton(page.root, "后台重读");
    await settle();
    expect(page.requests).toHaveLength(3);
    expect(statusRegion(page.root, WORKER_CONSISTENT, false)).toBe(region);
    expect(statusMutations).toEqual([]);
    changed.respond(configReadReply(snapshot));
    await settle();
    expect(statusRegion(page.root, WORKER_CONSISTENT, false)).toBe(region);
    expect(statusMutations).toEqual([]);
    expect(noticeLine(page.root, "alert", ".action-feedback--error", CONFLICT)).not.toBeNull();
    expectFieldKept();
    expect(button(page.root, "校验候选配置").hasAttribute("disabled")).toBe(true);
    expect(button(page.root, "写入配置").hasAttribute("disabled")).toBe(true);
    initial.input("60");
    flush();
    expect(initial.getAttribute("aria-invalid")).toBe("false");
    expect(noticeLine(page.root, "alert", ".action-feedback--error", CONFLICT)).not.toBeNull();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "GET", "GET"]);
  } finally {
    page.dispose();
  }
});

test("stale_config_callbacks_cannot_validate_or_write_until_the_baseline_is_current", async () => {
  const snapshot = forumProjection();
  const pending = deferredReply();
  const retried = deferredReply();
  const afterSave = deferredReply();
  const external = deferredReply();
  const reads = [
    () => configReadReply(snapshot),
    pending.wait,
    retried.wait,
    afterSave.wait,
    external.wait,
  ];
  let readsStarted = 0;
  const page = configDocumentPage((request) => {
    if (request.method === "GET") {
      const stage = required(reads[readsStarted], "读取回执");
      readsStarted += 1;
      return stage();
    }
    expect(request.method).toBe("PUT");
    return savedReply(snapshot, request);
  });
  const validation = () => button(page.root, "校验候选配置");
  const save = () => button(page.root, "写入配置");
  const confirm = () => button(page.root, "确认");
  try {
    page.mount();
    await settle();
    const form = formOf(page.root);
    const dispatchOldSubmit = captureEvent(form, "onSubmit");
    inputAt(page.root, LEASE).input("600");
    flush();
    const signature = setChecked(page.draft);
    const oldSave = save();
    expect(oldSave.hasAttribute("disabled")).toBe(false);
    const dispatchOldSave = captureEvent(oldSave, "onClick");
    const original = required(page.draft.fingerprint.peek(), "原指纹");
    page.draft.fingerprint.set(fingerprint("2"));
    expect(dispatchOldSubmit()).toBe(true);
    dispatchOldSave();
    expect(page.draft.validating.peek()).toBe(false);
    expect(page.draft.command.peek()).toBeNull();
    expect(page.draft.checkedSignature.peek()).toBe(signature);
    page.draft.fingerprint.set(original);
    await settle();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET"]);

    clickButton(page.root, "写入配置");
    await settle();
    const oldConfirm = confirm();
    expect(oldConfirm.hasAttribute("disabled")).toBe(false);
    const dispatchOldConfirm = captureEvent(oldConfirm, "onClick");
    const frozen = structuredClone(required(page.draft.command.peek(), "冻结命令"));
    refreshConfigDraft(page.draft);
    expect(dispatchOldSubmit()).toBe(true);
    dispatchOldConfirm();
    expect(page.draft.validating.peek()).toBe(false);
    expect(page.draft.checkedSignature.peek()).toBe(signature);
    await settle();
    expect(readsStarted).toBe(2);

    for (const reason of [LOADING, UNAVAILABLE]) {
      expectDialogStatus(page.root, reason);
      expect(validation().hasAttribute("disabled")).toBe(true);
      expect(save().hasAttribute("disabled")).toBe(true);
      expect(confirm().hasAttribute("disabled")).toBe(true);
      expect(captureEvent(form, "onSubmit")()).toBe(true);
      captureEvent(save(), "onClick")();
      captureEvent(confirm(), "onClick")();
      expect(page.draft.validating.peek()).toBe(false);
      await settle();
      expect(page.requests.filter(({ method }) => method !== "GET")).toEqual([]);
      expect(page.draft.command.peek()).toEqual(frozen);
      if (reason === LOADING) {
        pending.respond(apiFailure(503, "storage_unavailable", "暂不可用"));
        await settle();
      }
    }
    clickButton(page.root, "取消");
    await settle();
    expect(maybeButton(page.root, "确认")).toBeNull();
    clickButton(page.root, "重试");
    await settle();
    expect(readsStarted).toBe(3);
    retried.respond(configReadReply(snapshot));
    await settle();
    expect(validation().hasAttribute("disabled")).toBe(false);
    clickButton(page.root, "写入配置");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    expect(page.requests.filter(({ method }) => method === "PUT")).toHaveLength(1);
    expect(page.draft.fingerprint.peek()).toBe(fingerprint("1"));
    expect(readsStarted).toBe(4);
    afterSave.respond(configReadReply(snapshot));
    await settle();

    inputAt(page.root, LEASE).input("900");
    flush();
    setChecked(page.draft);
    clickButton(page.root, "写入配置");
    await settle();
    const externalConfirm = confirm();
    expect(externalConfirm.hasAttribute("disabled")).toBe(false);
    const dispatchExternalConfirm = captureEvent(externalConfirm, "onClick");
    const externalFrozen = structuredClone(required(page.draft.command.peek(), "第二次冻结命令"));
    expect(externalFrozen.fingerprint).toBe(fingerprint("1"));
    expect(valueAt(externalFrozen.document, LEASE)).toBe(900);
    snapshot.data.fingerprint = fingerprint("3");
    refreshConfigDraft(page.draft);
    await settle();
    expect(readsStarted).toBe(5);
    external.respond(configReadReply(snapshot));
    await settle();
    expectDialogStatus(page.root, CONFLICT);
    const disabledConfirm = confirm();
    expect(disabledConfirm.hasAttribute("disabled")).toBe(true);
    for (const dispatch of [dispatchExternalConfirm, captureEvent(disabledConfirm, "onClick")]) {
      dispatch();
      await settle();
      expect(page.requests).toHaveLength(6);
      expect(page.requests.filter(({ method }) => method === "PUT")).toHaveLength(1);
      expect(page.draft.command.peek()).toEqual(externalFrozen);
    }
    clickButton(page.root, "取消");
    await settle();
    expect(maybeButton(page.root, "确认")).toBeNull();
    clickButton(page.root, "重新加载当前配置");
    await settle();
    expect(button(page.root, "放弃修改并重新加载").hasAttribute("disabled")).toBe(false);
    expect(page.requests.map(({ method }) => method)).toEqual([
      "GET",
      "GET",
      "GET",
      "PUT",
      "GET",
      "GET",
    ]);
  } finally {
    page.dispose();
  }
});

test("config_save_and_restart_receipt_remain_visible_when_the_read_after_save_fails", async () => {
  const snapshot = forumProjection();
  snapshot.data.worker_restart_state = "restart_required";
  let reads = 0;
  const page = configDocumentPage((request) => {
    if (request.method === "PUT") {
      return savedReply(snapshot, request);
    }
    reads += 1;
    return reads === 2
      ? apiFailure(503, "storage_unavailable", "暂不可用")
      : configReadReply(snapshot);
  });
  const saved = "运行时配置已写入 TOML 文件。";
  const restart = "请重启受影响的进程，以应用尚未生效的配置。";
  try {
    page.mount();
    await settle();
    expect(page.root.textContent).not.toContain(saved);
    expect(page.root.textContent).not.toContain(restart);
    const region = statusRegion(page.root, WORKER_RESTART, true);
    const statusMutations = page.document.watchMounts(region);
    inputAt(page.root, LEASE).input("600");
    flush();
    setChecked(page.draft);
    clickButton(page.root, "写入配置");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    expect(reads).toBe(2);
    expect(noticeLine(page.root, "alert", ".ui-async-state__title", UNAVAILABLE)).not.toBeNull();
    expect(noticeLine(page.root, "status", ".action-feedback--success", saved)).not.toBeNull();
    expect(
      noticeLine(page.root, "status", ".admin-runtime-config__restart", restart),
    ).not.toBeNull();
    expect(button(page.root, "写入配置").hasAttribute("disabled")).toBe(true);
    expect(statusRegion(page.root, WORKER_RESTART, true)).toBe(region);
    expect(statusMutations).toEqual([]);

    snapshot.data.worker_restart_state = "consistent";
    clickButton(page.root, "重试");
    await settle();
    expect(reads).toBe(3);
    expect(page.root.textContent).not.toContain(UNAVAILABLE);
    expect(noticeLine(page.root, "status", ".action-feedback--success", saved)).not.toBeNull();
    expect(
      noticeLine(page.root, "status", ".admin-runtime-config__restart", restart),
    ).not.toBeNull();
    expect(statusRegion(page.root, WORKER_CONSISTENT, false)).toBe(region);
    expect(statusMutations).toEqual([]);
    expect(page.root.textContent).not.toContain(WORKER_RESTART);
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "PUT", "GET", "GET"]);
  } finally {
    page.dispose();
  }
});

test("config_validation_receipt_follows_the_session_draft_across_navigation", async () => {
  for (const [changeFingerprint, changeDraft, reject, returnBefore] of [
    [false, false, false, false],
    [false, false, true, true],
    [true, false, false, true],
    [false, true, true, true],
  ] as const) {
    const snapshot = forumProjection();
    const original = structuredClone(snapshot);
    const validation = deferredReply();
    const page = configDocumentPage((request) => {
      if (request.method === "GET") {
        return configReadReply(snapshot);
      }
      expect(request.method).toBe("POST");
      expect(valueAt(accepted(parseJson(request.body)), `document.${LEASE}`)).toBe(600);
      return validation.wait();
    });
    try {
      page.mount();
      await settle();
      inputAt(page.root, LEASE).input("600");
      flush();
      const signature = accepted(runtimeConfigFormSignature(page.draft.form.peek()));
      const originalForm = formOf(page.root);
      expect(originalForm.submit()).toBe(true);
      await settle();
      expect(page.requests.map(({ method }) => method)).toEqual(["GET", "POST"]);
      expect(page.draft.validating.peek()).toBe(true);
      page.unmount();
      expect(page.root.textContent).not.toContain("正在处理……");
      refreshConfigDraft(page.draft);
      if (changeFingerprint) {
        page.draft.form.set(required(page.draft.baseline.peek(), "原基线"));
        snapshot.data.fingerprint = fingerprint("2");
      }
      if (changeDraft) {
        page.draft.form.update((form) => runtimeConfigFormSetInput(form, LEASE, "601"));
      }
      if (returnBefore) {
        page.mount();
        await settle();
        expect(formOf(page.root)).not.toBe(originalForm);
        expect(button(page.root, "正在处理……").hasAttribute("disabled")).toBe(true);
      }
      if (changeFingerprint) {
        expect(page.draft.fingerprint.peek()).toBe(fingerprint("2"));
        expect(page.draft.fingerprint.peek()).not.toBe(original.data.fingerprint);
        page.draft.form.update((form) => runtimeConfigFormSetInput(form, LEASE, "600"));
        expect(accepted(runtimeConfigFormSignature(page.draft.form.peek()))).toBe(signature);
      }
      validation.respond(
        reject ? apiFailure(503, "storage_unavailable", "暂不可用") : Response.json(original),
      );
      await settle();
      expect(page.draft.validating.peek()).toBe(false);
      if (!returnBefore) {
        page.mount();
        await settle();
        expect(formOf(page.root)).not.toBe(originalForm);
      }
      if (changeFingerprint || changeDraft) {
        expect(page.draft.notice.peek()).toBeNull();
        expect(page.draft.checkedSignature.peek()).toBe("");
        expect(page.root.textContent).not.toContain(UNAVAILABLE);
      } else if (reject) {
        expect(page.draft.notice.peek()).toEqual({
          kind: "action-error",
          code: "storage_unavailable",
        });
        expect(page.draft.checkedSignature.peek()).toBe("");
        expect(
          noticeLine(page.root, "alert", ".action-feedback--error", UNAVAILABLE),
        ).not.toBeNull();
      } else {
        expect(page.draft.notice.peek()).toEqual({ kind: "validated" });
        expect(page.draft.checkedSignature.peek()).toBe(signature);
        expect(
          noticeLine(page.root, "status", ".action-feedback--success", "候选已校验，可以写入。"),
        ).not.toBeNull();
      }
      expect(button(page.root, "写入配置").hasAttribute("disabled")).toBe(
        reject || changeFingerprint || changeDraft,
      );
      expect(page.requests.map(({ method }) => method)).toEqual(["GET", "POST", "GET"]);
    } finally {
      page.dispose();
    }
  }
});

test("hot_forum_timeout_save_preserves_the_previous_lease_restart_notice", () => {
  const initial = decodeConfigData(forumProjection().data);
  const scope = cleanupScope();
  try {
    const draft = createRoot((stop) => {
      scope.defer(stop);
      return useConfigDraft();
    });
    const startup = accepted(runtimeConfigFormFromResponse(initial));
    draft.baseline.set(startup);
    draft.form.set(startup);
    draft.fingerprint.set(initial.fingerprint);
    for (const [field, value, needsRestart, digit] of [
      ["lease_duration_seconds", 600, true, "1"],
      ["item_timeout_seconds", 360, false, "2"],
    ] as const) {
      expect(draft.restartRequired.peek()).toBe(!needsRestart);
      const source = runtimeConfigFormSourceDocument(required(draft.baseline.peek(), "基线"));
      const next = structuredClone(source);
      objectAt(next, "operations.forum")[field] = value;
      const document = documentValue(next);
      const changed = configChanges(source, document);
      expect(changed).toHaveLength(1);
      expect(changed.some(configChangeNeedsRestart)).toBe(needsRestart);
      const result = {
        ...initial,
        equipment_taxonomy: initial.equipment_taxonomy ?? null,
        document,
        fingerprint: fingerprint(digit),
        restart_required: true,
        worker_restart_state: null,
      };
      draft.checkedSignature.set("已校验的候选");
      draft.command.set({ fingerprint: required(draft.fingerprint.peek()), document });
      draft.applySavedConfig(result, accepted(runtimeConfigFormFromResponse(result)), needsRestart);
      expect(draft.restartRequired.peek()).toBe(true);
      expect(runtimeConfigFormSourceDocument(required(draft.baseline.peek()))).toEqual(document);
      expect(runtimeConfigFormSourceDocument(draft.form.peek())).toEqual(document);
      expect(draft.fingerprint.peek()).toBe(fingerprint(digit));
      expect(draft.checkedSignature.peek()).toBe("");
      expect(draft.command.peek()).toBeNull();
      expect(draft.notice.peek()).toBeNull();
    }
    draft.rememberRestartNotice(false);
    expect(draft.restartRequired.peek()).toBe(true);
  } finally {
    scope.dispose();
  }
});
