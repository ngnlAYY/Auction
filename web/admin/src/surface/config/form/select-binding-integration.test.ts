// 运行配置下拉框的挂载、预校验回填与暂停确认取消回归。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { createRoot, flush } from "solid-js";
import {
  installBrowserRequest,
  installWriteFetch,
  type WriteReply,
} from "../../../api/api-request-fixture";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { cleanupScope } from "../../../test-scope-fixture";
import { PendingCommandHostContext } from "../../pending";
import { createPendingHarness } from "../../pending/pending-command-fixture";
import {
  createComponent,
  createElement,
  findElement,
  insert,
  installTestDocument,
  required,
  type TestElement,
} from "../../solid-dom-fixture";
import { loadConfigFixture } from "../config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "../config-test-scope-fixture.test";
import { ConfigDraftContext, useConfigDraft } from "../draft";
import {
  runtimeConfigFormDefault,
  runtimeConfigFormInput,
  runtimeConfigFormSetInput,
} from "../model";

const [
  { SelectField },
  { AdminSessionHostContext },
  {
    document: { RuntimeConfigDocument },
    runtimeSwitches: { RuntimeSwitchCard },
  },
] = await Promise.all([
  loadRenderModule<typeof import("./fields")>(
    new URL("./fields.tsx", import.meta.url).pathname,
    "SelectField",
  ),
  loadRenderModule<typeof import("../../auth/shell")>(
    new URL("../../auth/shell.tsx", import.meta.url).pathname,
    "AdminSessionHostContext",
  ),
  loadConfigFixture(),
]);
const FIXTURES = new URL(
  "../../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const LISTENER_MODE = "listener.mode";
const PREVIEW_DAY = "operations.schedule.preview_day";
const scope = cleanupScope();

beforeEach(beginConfigTestLock);
afterEach(() => {
  try {
    scope.dispose();
  } finally {
    endConfigTestLock();
  }
});

function mount(view: () => JSX.Element): TestElement {
  const document = installTestDocument();
  scope.defer(document.restore);
  const root = createElement("div");
  document.add(root);
  createRoot((dispose) => {
    scope.defer(dispose);
    insert(root, view);
  });
  flush();
  return root;
}

function mountAdmin(view: () => JSX.Element, reply: WriteReply) {
  const pending = createPendingHarness();
  scope.defer(pending.dispose);
  scope.defer(installBrowserRequest());
  const fetch = installWriteFetch(reply);
  scope.defer(fetch.restore);
  const root = mount(() =>
    createComponent(AdminSessionHostContext, {
      value: pending.host.sessionHost,
      get children() {
        return createComponent(PendingCommandHostContext, {
          value: pending.host,
          get children() {
            return view();
          },
        });
      },
    }),
  );
  return { root, requests: fetch.writes };
}

function jsonObject(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("配置样本缺少 JSON 对象");
  }
  // 样本已通过 parseJson，剩下的容器均为可写 JSON 对象。
  return value as Record<string, unknown>;
}

function readProjection(name: string) {
  const parsed = parseJson(readFileSync(new URL(name, FIXTURES), "utf8"));
  if (!parsed.ok) {
    throw new Error(`无法解析配置样本 ${name}`);
  }
  const envelope = jsonObject(parsed.value);
  return { ...envelope, data: jsonObject(envelope.data) };
}

function configProjection() {
  const projection = readProjection("runtime-config-default.json");
  const document = jsonObject(projection.data.document);
  Object.assign(jsonObject(document.listener), {
    mode: "unix",
    address: "",
    socket_path: "/run/hv-auction/admin.sock",
  });
  document.dashboard_mailboxes = [{ world_id: "isekai", mailbox_key: "fixture-box" }];
  jsonObject(projection.data.equipment_taxonomy).resolved_kinds = [];
  jsonObject(jsonObject(document.operations).schedule).managed = true;
  return projection;
}

async function settle(): Promise<void> {
  for (let round = 0; round < 12; round += 1) {
    await Promise.resolve();
    flush();
  }
}

function named(root: TestElement, tag: string, name: string): TestElement | null {
  return findElement(root, (node) => node.tag === tag && node.getAttribute("name") === name);
}

function select(root: TestElement, name: string): TestElement {
  return required(named(root, "select", name), name);
}

function selection(select: TestElement): [string, string | undefined] {
  const value = select.value;
  return [
    value,
    select.querySelectorAll("option").find((option) => option.value === value)?.textContent,
  ];
}

test("配置投影首次显示 Unix 和周四，预校验往返期间保持选中值", async () => {
  const projection = configProjection();
  const receipt = Promise.withResolvers<Response>();
  const page = mountAdmin(
    () =>
      createComponent(ConfigDraftContext, {
        value: useConfigDraft(),
        get children() {
          return createComponent(RuntimeConfigDocument, {});
        },
      }),
    (request) => {
      if (request.method === "GET" && request.path === "/api/v1/admin/config") {
        return Response.json({
          ...projection,
          data: { ...projection.data, worker_restart_state: "unknown" },
        });
      }
      if (request.method === "POST" && request.path === "/api/v1/admin/config:validate") {
        return receipt.promise;
      }
      throw new Error(`意外请求 ${request.method} ${request.path}`);
    },
  );
  await settle();

  const mode = select(page.root, LISTENER_MODE);
  const day = select(page.root, PREVIEW_DAY);
  expect(selection(mode)).toEqual(["unix", "Unix 套接字"]);
  expect(selection(day)).toEqual(["Thursday", "星期四"]);
  expect(select(page.root, "mailboxes-0-world").value).toBe("isekai");
  required(named(page.root, "input", "listener.socket_path"), "Unix 套接字路径").input(
    "/run/hv-auction/new-admin.sock",
  );
  flush();

  required(page.root.querySelector("form"), "配置表单").submit();
  await settle();
  expect(mode.hasAttribute("disabled")).toBe(true);
  expect(day.hasAttribute("disabled")).toBe(true);
  expect(mode.value).toBe("unix");
  expect(day.value).toBe("Thursday");
  const sent = required(
    page.requests.find((request) => request.method === "POST"),
    "预校验请求",
  );
  expect(parseJson(sent.body)).toMatchObject({
    ok: true,
    value: {
      document: {
        listener: { mode: "unix", socket_path: "/run/hv-auction/new-admin.sock" },
        operations: { schedule: { preview_day: "Thursday" } },
      },
    },
  });

  receipt.resolve(Response.json(projection));
  await settle();
  expect(mode.hasAttribute("disabled")).toBe(false);
  expect(day.hasAttribute("disabled")).toBe(false);
  expect(select(page.root, LISTENER_MODE).value).toBe("unix");
  expect(select(page.root, PREVIEW_DAY).value).toBe("Thursday");
  expect(page.root.textContent).toContain("候选已校验，可以写入。");
});

type Options = readonly (readonly [string, string])[];

function mountSelect(initial: string, initialOptions: Options) {
  const form = createCell(
    runtimeConfigFormSetInput(runtimeConfigFormDefault(), LISTENER_MODE, initial),
  );
  const options = createCell(initialOptions);
  const root = mount(() =>
    createComponent(SelectField, {
      form,
      path: LISTENER_MODE,
      label: "监听器模式",
      disabled: false,
      get options() {
        return options();
      },
      onDirty() {},
    }),
  );
  return { form, options, select: select(root, LISTENER_MODE) };
}

const LISTENER_OPTIONS: Options = [
  ["tcp", "TCP"],
  ["unix", "Unix 套接字"],
];

test("草稿从 TCP 外部回填为 Unix 后显示 Unix", () => {
  const field = mountSelect("tcp", LISTENER_OPTIONS);
  expect(field.select.value).toBe("tcp");
  field.form.update((form) => runtimeConfigFormSetInput(form, LISTENER_MODE, "unix"));
  flush();
  expect(field.select.value).toBe("unix");
});

test("选项缺少当前值时显示首项，新增或换序后仍按草稿选中", async () => {
  const field = mountSelect("unix", [["tcp", "TCP"]]);
  expect(selection(field.select)).toEqual(["tcp", "TCP"]);
  expect(runtimeConfigFormInput(field.form.peek(), LISTENER_MODE)).toBe("unix");
  field.options.set(LISTENER_OPTIONS);
  await settle();
  expect(field.select.value).toBe("unix");
  field.options.set([["extra", "其他"], ...LISTENER_OPTIONS]);
  await settle();
  expect(field.select.value).toBe("unix");
  field.options.set([["tcp", "TCP"]]);
  await settle();
  expect(selection(field.select)).toEqual(["tcp", "TCP"]);
  expect(runtimeConfigFormInput(field.form.peek(), LISTENER_MODE)).toBe("unix");
});

test("选择 6 小时后打开确认并返回，重挂的暂停时长仍显示 6 小时", async () => {
  const snapshot = readProjection("runtime-switches.json");
  // 使用已过期的暂停投影，卡片应允许重新选择暂停时长。
  snapshot.data.database_time = "2025-11-15T11:00:00Z";
  const { root, requests } = mountAdmin(
    () => createComponent(RuntimeSwitchCard, {}),
    (request) => {
      if (request.method === "GET" && request.path === "/api/v1/admin/runtime-switches") {
        return Response.json(snapshot);
      }
      throw new Error(`意外请求 ${request.method} ${request.path}`);
    },
  );
  await settle();
  const initial = select(root, "duration");
  expect(initial.value).toBe("2");
  initial.change("6");
  required(named(root, "input", "reason"), "暂停原因").input("测试暂停时长");
  required(root.querySelector("form"), "暂停表单").submit();
  await settle();
  expect(named(root, "select", "duration")).toBeNull();
  required(
    findElement(root, (node) => node.tag === "button" && node.textContent === "返回"),
    "取消暂停确认按钮",
  ).click();
  await settle();
  const restored = select(root, "duration");
  expect(restored).not.toBe(initial);
  expect(selection(restored)).toEqual(["6", "6 h"]);
  expect(requests.map((request) => request.method)).toEqual(["GET"]);
});
