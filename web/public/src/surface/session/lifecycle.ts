// 会话外部变化监听与有限会话截止检查。
// 聚焦与同源通知由外壳安排权威重读，截止计时独立于页面展示的读取结果。

import { sleep } from "@auction/web-shared/platform/timer";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { onCleanup } from "solid-js";

import { estimatedNow } from "../../api/clock";
import {
  PUBLIC_SESSION_CHANGE_STORAGE_KEY,
  type PublicSession,
  parseStrictRfc3339,
  type SessionRefreshReason,
} from "../../api/session";

export function usePublicSessionSync(checkSession: (reason: SessionRefreshReason) => void): void {
  if (typeof window === "undefined") {
    return;
  }
  const storage = (event: StorageEvent): void => {
    if (event.key === PUBLIC_SESSION_CHANGE_STORAGE_KEY && event.newValue !== null) {
      // 修订不证明身份；另一标签页刚写过 Cookie，只读一次当前会话。
      checkSession("cookie_changed");
    }
  };
  const focus = (): void => {
    checkSession("refresh");
  };
  window.addEventListener("storage", storage);
  window.addEventListener("focus", focus);
  onCleanup(() => {
    window.removeEventListener("storage", storage);
    window.removeEventListener("focus", focus);
  });
}

/** 有限会话只在真实截止后复核一次；永久会话不建立计时器，也不把远期日期当永久。 */
export function usePublicSessionExpiry(
  confirmedIdentity: () => PublicSession | null,
  onExpiry: () => void,
): void {
  // 同一身份及截止的重复快照不重启已完成的任务，避免服务端时钟误差造成忙轮询。
  const deadline = createValueMemo((): readonly [string, bigint] | null => {
    const identity = confirmedIdentity();
    if (identity === null || identity.expires_at === null) {
      return null;
    }
    const expiry = parseStrictRfc3339(identity.expires_at);
    return expiry === null ? null : ([identity.user_id, expiry] as const);
  });
  createTaskResource(deadline, async (current, context) => {
    if (current === null) {
      return;
    }
    const expiry = current[1];
    while (context.alive()) {
      // Date 头只有秒精度；给出一秒余量，长截止分段等待以免浏览器 i32 超时溢出。
      const remaining = (expiry - BigInt(estimatedNow()) * 1_000_000n) / 1_000_000n + 1_000n;
      if (remaining <= 0n) {
        onExpiry();
        return;
      }
      const wait = remaining > 2_147_483_647n ? 2_147_483_647 : Number(remaining);
      if (!(await sleep(wait, context.signal))) {
        return;
      }
      if (!context.alive()) {
        return;
      }
    }
  });
}
