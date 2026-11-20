// 邮箱轮询用例中的安全快照与会话回执挂载。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import type { AdminMailPolling } from "../../api/mail";
import { requirePendingCommandHost } from "../pending";
import { PendingCommandNotices } from "../pending/panel";
import { createCommandReload } from "../pending/sources";
import { MailPollingCard, MailPollingCommands } from "./actions";
import { page } from "./page";

/** Rust 测试中的邮箱读取快照。 */
export function polling(mailboxKey: string): AdminMailPolling {
  return {
    mailbox_key: mailboxKey,
    paused: false,
    state_version: 1,
    consecutive_failures: 0,
    last_poll_at: "2026-11-18T00:00:00Z",
    next_poll_at: "2026-11-18T00:01:00Z",
    updated_at: "2026-11-18T00:00:00Z",
    diagnostics: null,
  };
}

/** 暂停回执走旧投影，Rust 序列化时省略没有值的 diagnostics。 */
export function pausedPollingWire() {
  const { diagnostics: _, ...state } = polling("mail-a");
  return { ...state, paused: true, state_version: 2 };
}

/** 单次挂载真实邮箱命令组件，切箱通过按钮且按邮箱重挂卡片。 */
export function PollingActionsFixture(): JSX.Element {
  const host = requirePendingCommandHost();
  const refresh = createCommandReload();
  const mailbox = createCell("mail-a");
  return (
    <>
      <PendingCommandNotices host={host} />
      <button type="button" onClick={() => mailbox.set("mail-b")}>
        切换邮箱
      </button>
      <p id="test-poll-generation">轮询世代 {String(refresh())}</p>
      <MailPollingCommands refresh={refresh} activeMailbox={mailbox}>
        <For each={[mailbox()]} keyed={(key) => key}>
          {(key) => (
            <MailPollingCard polling={polling(key())} refresh={refresh} loadedGeneration={0n} />
          )}
        </For>
      </MailPollingCommands>
    </>
  );
}

/** 原页面和认证外壳的回执区共享同一个会话宿主。 */
export function MailPageFixture(): JSX.Element {
  const host = requirePendingCommandHost();
  return (
    <>
      <PendingCommandNotices host={host} />
      {page()}
    </>
  );
}
