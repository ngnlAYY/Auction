// 未决面板：关闭首条回执保留后续按钮，显式移除后焦点落到下一记录再回主区。

import { expect, jest, spyOn, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createComponent, createRoot, flush } from "solid-js";
import { writeFailureFromReply } from "../../api/api-request-fixture";
import type { AdminWriteFailure } from "../../api/failure";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import {
  click,
  createElement,
  findElement,
  insert,
  installTestDocument,
  type TestElement,
} from "../solid-dom-fixture";
import { type CommandIdentity, type CommandNotice, PendingCommandHostContext } from ".";
import { settlePending } from "./pending-command-fixture";
import { createSessionHost } from "./session-fixture";
import { createCommandReload, usePendingCommands } from "./sources";

const { PendingCommandNotices } = await loadRenderModule<typeof import("./panel")>(
  new URL("panel.tsx", import.meta.url).pathname,
  "PendingCommandNotices",
);
const UNAVAILABLE_BODY =
  '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}';

function identity(commandId: string): CommandIdentity {
  return { entity: commandId, action: "测试操作", commandId };
}

function buttonByText(tree: TestElement, text: string): TestElement {
  const button = tree.querySelectorAll("button").find((node) => node.textContent.trim() === text);
  if (button === undefined) throw new Error(`缺少按钮 ${text}`);
  return button;
}

test("关闭首条回执时保留第二条按钮", () => {
  const scope = cleanupScope();
  try {
    scope.defer(installAdminTranslation());
    scope.defer(flush);
    const { host, dispose } = createSessionHost();
    scope.defer(dispose);
    const document = installTestDocument();
    scope.defer(() => document.restore());

    const first = "00000000-0000-0000-0000-000000000001";
    const second = "00000000-0000-0000-0000-000000000002";
    host.notices.set([
      { identity: identity(first), result: ok(null), keySource: null },
      { identity: identity(second), result: ok(null), keySource: null },
    ] satisfies readonly CommandNotice[]);

    const tree = createElement("div");
    createRoot((disposeRoot) => {
      scope.defer(disposeRoot);
      insert(tree, () => createComponent(PendingCommandNotices, { host }));
    });
    document.add(tree);
    flush();

    const secondButtonId = `admin-command-receipt-${second}`;
    const secondButton = document.getElementById(secondButtonId);
    if (secondButton === null) throw new Error("第二条回执按钮未挂载");
    const listenerWrites = () => secondButton.propWrites.filter(({ key }) => key === "onClick");
    const before = listenerWrites();
    expect(before).toHaveLength(1);

    click(buttonByText(tree, "关闭回执"));
    flush();
    expect(buttonByText(tree, "关闭回执")).toBe(secondButton);
    expect(document.getElementById(secondButtonId)).toBe(secondButton);
    expect(listenerWrites()).toEqual(before);
    expect(document.getElementById(`admin-command-receipt-${first}`)).toBeNull();

    click(secondButton);
    flush();
    expect(host.notices.peek()).toEqual([]);
  } finally {
    scope.dispose();
  }
});

test("显式移除先把焦点移到下一条记录再回主区", async () => {
  for (const { unknown, mainAvailable } of [
    { unknown: true, mainAvailable: true },
    { unknown: false, mainAvailable: true },
    { unknown: true, mainAvailable: false },
    { unknown: false, mainAvailable: false },
  ]) {
    const scope = cleanupScope();
    try {
      const replies: [string, Result<void, AdminWriteFailure>][] = [];
      for (const entity of ["world:one", "world:two"]) {
        const result = unknown
          ? err(await writeFailureFromReply(503, UNAVAILABLE_BODY))
          : ok(undefined);
        replies.push([entity, result]);
      }
      scope.defer(installAdminTranslation());
      scope.defer(flush);
      jest.useFakeTimers();
      scope.defer(() => jest.useRealTimers());
      const { host, dispose } = createSessionHost();
      scope.defer(dispose);
      const document = installTestDocument();
      scope.defer(() => document.restore());
      const main = createElement("main", { id: "admin-content", tabindex: "-1" });
      const submissions: boolean[] = [];
      createRoot((disposeRoot) => {
        scope.defer(disposeRoot);
        insert(main, () =>
          createComponent(PendingCommandHostContext, {
            value: host,
            get children() {
              const owner = usePendingCommands(createCommandReload(0n), () => true);
              const send = createElement("button", {
                type: "button",
                onClick() {
                  for (const [entity, result] of replies) {
                    submissions.push(
                      owner.submitWithReceipt(
                        entity,
                        "测试操作",
                        async () => result,
                        () => ({ kind: "receipt", receipt: null }),
                        () => {},
                      ),
                    );
                  }
                },
              });
              insert(send, "发送两条命令");
              return send;
            },
          }),
        );
        insert(main, createComponent(PendingCommandNotices, { host }));
      });
      document.add(main);
      flush();

      const lookup = spyOn(document, "getElementById");
      scope.defer(() => lookup.mockRestore());
      const focusIds = () => document.focusCalls.map(({ id }) => id);
      const press = async (label: string): Promise<readonly string[]> => {
        click(buttonByText(main, label));
        await settlePending();
        lookup.mockClear();
        jest.advanceTimersByTime(0);
        await settlePending();
        return lookup.mock.calls.map(([id]) => id);
      };

      expect(await press("发送两条命令")).toEqual([]);
      expect(submissions).toEqual([true, true]);
      const records = unknown ? host.entries.peek() : host.notices.peek();
      expect(records).toHaveLength(2);
      if (unknown) {
        expect(host.entries.peek().map(({ error }) => error)).toEqual([
          "storage_unavailable",
          "storage_unavailable",
        ]);
      }
      const second = records[1];
      if (second === undefined) throw new Error("缺少第二条命令记录");
      expect(document.activeElement).toBeNull();
      expect(focusIds()).toEqual([]);

      const buttonLabel = unknown ? "放弃" : "关闭回执";
      const removeLabel = unknown ? "确认放弃" : buttonLabel;
      if (unknown) {
        await press(buttonLabel);
        expect(await press("保留未决操作")).toEqual([]);
        expect(document.activeElement).toBeNull();
        expect(focusIds()).toEqual([]);
        await press(buttonLabel);
      }

      const next = `admin-command-${unknown ? "retry" : "receipt"}-${second.identity.commandId}`;
      const firstLookups = await press(removeLabel);
      expect(firstLookups[0]).toBe(next);
      expect(focusIds()).toEqual([next]);
      expect(document.activeElement?.id).toBe(next);

      if (unknown) {
        await press(buttonLabel);
      }
      if (!mainAvailable) {
        // 模拟查找落空，让循环查完整个末次候选列表。
        lookup.mockReturnValue(null);
      }
      const lastLookups = await press(removeLabel);
      expect(lastLookups[0]).toBe("admin-content");
      if (!mainAvailable) {
        expect(lastLookups).toEqual(["admin-content"]);
      }
      expect(focusIds()).toEqual(mainAvailable ? [next, "admin-content"] : [next]);
      expect(document.activeElement?.id).toBe(mainAvailable ? "admin-content" : next);
      expect(findElement(main, (element) => element.textContent.trim() === buttonLabel)).toBeNull();
    } finally {
      scope.dispose();
    }
  }
});
