// 未决命令测试的会话、宿主与来源页替身。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createEffect, createRoot, flush } from "solid-js";
import type { AdminSession } from "../../api/session";
import { registerAdminSessionHost } from "../../api/session";
import { cleanupScope } from "../../test-scope-fixture";
import {
  type CommandNotice,
  createPendingCommands,
  type PendingCommandGate,
  type PendingCommandHost,
  type PendingCommands,
  usePendingCommandGate,
  usePendingCommandHost,
} from ".";
import { type CommandSource, createCommandReload } from "./sources";

/** 已挂载来源页及其命令入口。 */
export interface PendingSourceFixture {
  readonly reload: Cell<bigint>;
  readonly gate: PendingCommandGate | null;
  readonly rejections: Cell<readonly CommandNotice[]>;
  readonly source: CommandSource;
  readonly commands: PendingCommands;
  detach(): void;
}

/** 一次测试用的认证宿主与来源页。 */
export interface PendingHarness {
  readonly host: PendingCommandHost;
  readonly origin: PendingSourceFixture;
  setWritesAllowed(value: boolean): void;
  registerSource(gate: PendingCommandGate | null): PendingSourceFixture;
  dispose(): void;
}

const SESSION: AdminSession = Object.freeze({
  actor: "admin",
  csrfToken: "csrf-token",
  csrf_token: "csrf-token",
  expiresAt: 4_000_000_000n,
  absoluteExpiresAt: 4_000_000_000n,
  serverTime: 1_800_000_000n,
  receivedAtMillis: 1_800_000_000_000,
});

/** 安装写入许可可变的会话宿主，并登记一个可选门禁的来源页。 */
export function createPendingHarness(options: { readonly gate?: boolean } = {}): PendingHarness {
  let writesAllowed = true;
  const registration = registerAdminSessionHost({
    check: () => {},
    writesAllowed: () => writesAllowed,
    confirmed: () => SESSION,
  });
  const scope = cleanupScope();
  scope.defer(registration.release);
  try {
    const { host: installedHost, gate } = createRoot((dispose) => {
      scope.defer(dispose);
      return {
        host: usePendingCommandHost(registration.host),
        gate: options.gate === true ? usePendingCommandGate() : null,
      };
    });
    const registerSource = (sourceGate: PendingCommandGate | null): PendingSourceFixture => {
      const reload = createCommandReload(0n);
      const rejections = createCell<readonly CommandNotice[]>([]);
      const sourceCell = createCell<CommandSource>({
        reload,
        gate: sourceGate,
        rejections,
        matchesEntity: () => true,
      });
      const source = sourceCell.peek();
      installedHost.sources.update((sources) => [...sources, new WeakRef(source)]);
      const commands = createPendingCommands(installedHost, sourceCell);
      let attached = true;
      return {
        reload,
        gate: sourceGate,
        rejections,
        source,
        commands,
        detach() {
          if (!attached) {
            return;
          }
          attached = false;
          installedHost.sources.update((sources) =>
            sources.filter((candidate) => candidate.deref() !== source),
          );
        },
      };
    };

    const origin = registerSource(gate);
    return {
      host: installedHost,
      origin,
      setWritesAllowed(value) {
        writesAllowed = value;
      },
      registerSource,
      dispose: scope.dispose,
    };
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

/** 让宿主任务跨过微任务边界，并刷新 Solid 的响应式计算。 */
export async function settlePending(): Promise<void> {
  for (let round = 0; round < 6; round += 1) {
    await Promise.resolve();
    flush();
  }
}

/** 宿主公布尝试完成后解除等待；未完成的尝试由测试运行器超时。 */
export async function waitForAttempt(isBusy: () => boolean): Promise<void> {
  const completed = Promise.withResolvers<void>();
  const dispose = createRoot((dispose) => {
    createEffect(isBusy, (busy) => {
      if (!busy) {
        completed.resolve();
      }
    });
    return dispose;
  });
  try {
    flush();
    await completed.promise;
  } finally {
    dispose();
  }
}
