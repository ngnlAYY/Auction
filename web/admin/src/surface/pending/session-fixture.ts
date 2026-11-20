// 会话导航测试的认证宿主：只创建外壳命令宿主，不预先登记来源页。

import { createRoot } from "solid-js";

import type { AdminSession } from "../../api/session";
import { registerAdminSessionHost } from "../../api/session";
import { type PendingCommandHost, usePendingCommandHost } from ".";

const SESSION: AdminSession = Object.freeze({
  actor: "fixture",
  csrfToken: "csrf-fixture",
  csrf_token: "csrf-fixture",
  expiresAt: 4_000_000_000n,
  absoluteExpiresAt: 4_000_000_000n,
  serverTime: 1_800_000_000n,
  receivedAtMillis: 1_800_000_000_000,
});

/** 一次测试的认证会话宿主。 */
export interface SessionHostFixture {
  readonly host: PendingCommandHost;
  setWritesAllowed(value: boolean): void;
  dispose(): void;
}

/** 安装只含外壳命令宿主的会话；来源页由页面组件自行登记。 */
export function createSessionHost(): SessionHostFixture {
  let writesAllowed = true;
  const registration = registerAdminSessionHost({
    check: () => {},
    writesAllowed: () => writesAllowed,
    confirmed: () => SESSION,
  });
  let disposeRoot = (): void => {};
  const dispose = (): void => {
    try {
      disposeRoot();
    } finally {
      registration.release();
    }
  };
  try {
    const host = createRoot((disposeCurrentRoot) => {
      disposeRoot = disposeCurrentRoot;
      return usePendingCommandHost(registration.host);
    });
    return {
      host,
      setWritesAllowed(value) {
        writesAllowed = value;
      },
      dispose,
    };
  } catch (error) {
    dispose();
    throw error;
  }
}
