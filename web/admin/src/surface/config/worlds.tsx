// 数据库权威业务世界状态控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import {
  type AdminBusinessWorld,
  fetchAdminBusinessWorlds,
  prepareAdminBusinessWorldSetEnabledOnce,
  setAdminBusinessWorldEnabled,
} from "../../api/worlds";
import { useAdminSessionHost } from "../auth/shell";
import {
  PendingCommandsContext,
  requirePendingCommands,
  setIfAlive,
  usePendingCommandGate,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { createCommandReload, usePendingCommandsWithGate } from "../pending/sources";
import { adminApiErrorMessage, adminWorldLabel } from "../presentation";

/** Rust `u64` 的上界；读取世代与 `saturating_add(1)` 都不得越过它。 */
const U64_MAX = 18_446_744_073_709_551_615n;

/** 一次启停命令的反馈；`error_code` 为 null 表示成功。 */
interface BusinessWorldNotice {
  readonly worldId: string;
  readonly enabled: boolean;
  readonly errorCode: string | null;
}

/** 一次读取的世代与其业务世界列表；世代用于判断卡片的快照是否仍为当前读取。 */
type WorldsRead = readonly [bigint, Result<readonly AdminBusinessWorld[], string>];

/** 推进读取世代，并在 `u64::MAX` 饱和。 */
function advanceRefresh(refresh: Cell<bigint>): void {
  const current = refresh.peek();
  refresh.set(current === U64_MAX ? current : current + 1n);
}

/** 数据库权威业务世界状态列表。 */
export function AdminBusinessWorlds(): JSX.Element {
  const host = useAdminSessionHost();
  const refresh = createCommandReload(0n);
  const notice = createCell<BusinessWorldNotice | null>(null);
  const commandGate = usePendingCommandGate();
  const pending = usePendingCommandsWithGate(refresh, commandGate, (entity) =>
    entity.startsWith("world:"),
  );
  const snapshot = createTaskResource<bigint, WorldsRead>(
    () => refresh(),
    async (generation, context): Promise<WorldsRead> => {
      let result: Result<readonly AdminBusinessWorld[], string>;
      try {
        result = await fetchAdminBusinessWorlds(host, context.signal);
      } catch {
        // 传输边界异常可能含秘密；只按安全的传输失败码收尾。
        result = err("network_error");
      }
      return [generation, result];
    },
  );

  const current = (): WorldsRead | undefined => snapshot();
  const failure = (): { readonly code: string } | null => {
    const entry = current();
    return entry !== undefined && !entry[1].ok ? { code: entry[1].error } : null;
  };
  const loaded = (): {
    readonly generation: bigint;
    readonly worlds: readonly AdminBusinessWorld[];
  } | null => {
    const entry = current();
    if (entry === undefined || !entry[1].ok) {
      return null;
    }
    return { generation: entry[0], worlds: entry[1].value };
  };

  return (
    <PendingCommandsContext value={pending}>
      <section
        class="admin-business-worlds admin-panel"
        aria-labelledby="admin-business-worlds-title"
      >
        <h2 id="admin-business-worlds-title">{t("admin-business-worlds-title")}</h2>
        <p class="admin-panel__note">{t("admin-business-worlds-note")}</p>
        <PendingCommandPanel owner={pending} />
        <Switch
          fallback={
            <AsyncState kind="loading" title={t("admin-business-worlds-loading")} detail="" />
          }
        >
          <Match when={failure()}>
            {(failed) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(failed().code)}
                detail=""
                retryLabel={t("admin-retry")}
                // 共享面板没有 disabled 态；繁忙时重试点击在回调里直接忽略。
                onRetry={() => {
                  if (!pending.anyBusy()) {
                    advanceRefresh(refresh);
                  }
                }}
              />
            )}
          </Match>
          <Match when={loaded()}>
            {(entry) => (
              <div class="admin-business-worlds__grid">
                <For each={entry().worlds} keyed={(world) => `${world.id}:${world.policyRevision}`}>
                  {(world) => (
                    <BusinessWorldCard
                      world={world()}
                      notice={notice}
                      refresh={refresh}
                      loadedGeneration={entry().generation}
                    />
                  )}
                </For>
              </div>
            )}
          </Match>
        </Switch>
        <Show when={notice()}>
          {(currentNotice) => (
            <p
              class={[
                "admin-panel__note",
                "action-feedback",
                {
                  "action-feedback--success": currentNotice().errorCode === null,
                  "action-feedback--error": currentNotice().errorCode !== null,
                },
              ]}
              role={currentNotice().errorCode === null ? "status" : "alert"}
            >
              <Show
                when={currentNotice().errorCode !== null}
                fallback={
                  currentNotice().enabled
                    ? t("admin-business-world-enabled-success", {
                        world: adminWorldLabel(currentNotice().worldId),
                      })
                    : t("admin-business-world-disabled-success", {
                        world: adminWorldLabel(currentNotice().worldId),
                      })
                }
              >
                {adminApiErrorMessage(currentNotice().errorCode ?? "")}
              </Show>
            </p>
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}

/** 单个业务世界卡片；只有当前读取的策略版本才能开启或确认新的启停命令。 */
function BusinessWorldCard(props: {
  readonly world: AdminBusinessWorld;
  readonly notice: Cell<BusinessWorldNotice | null>;
  readonly refresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}): JSX.Element {
  const host = useAdminSessionHost();
  const pending = requirePendingCommands();
  const targetEnabled = () => !props.world.enabled;
  const worldId = () => props.world.id;
  const worldName = () => adminWorldLabel(props.world.id);
  const entity = () => `world:${props.world.id}`;
  const actionBusy = () => pending.isBusy(entity());
  // 旧快照可展示。只有当前读取的策略版本才能开启或确认新的启停命令。
  const readIsCurrent = () => props.refresh() === props.loadedGeneration;
  const blocked = () => !readIsCurrent() || pending.anyBusy() || pending.contains(entity());
  const availability = () => availabilityLabel(props.world.availability);
  const actionLabel = () =>
    actionBusy()
      ? t("admin-action-working")
      : props.world.enabled
        ? t("admin-business-world-disable")
        : t("admin-business-world-enable");
  // 启停是危险操作：按钮只打开确认对话框，真正的 mutation 由对话框确认按钮触发。
  const confirmOpen = createCell(false);

  const openConfirm = (): void => {
    if (!readIsCurrent() || pending.anyBusy() || pending.contains(entity())) {
      return;
    }
    props.notice.set(null);
    confirmOpen.set(true);
  };

  const submit = (): void => {
    if (!readIsCurrent() || pending.anyBusy() || pending.contains(entity())) {
      return;
    }
    const frozenWorldId = worldId();
    const target = targetEnabled();
    // 冻结确认那一刻的世界、目标状态与策略围栏；重试只复用这些值。
    const prepare = prepareAdminBusinessWorldSetEnabledOnce(
      frozenWorldId,
      target,
      props.world.policyRevision,
    );
    const submitted = pending.submit<AdminBusinessWorld>(
      entity(),
      target ? t("admin-business-world-enable") : t("admin-business-world-disable"),
      async (commandId, signal) => {
        try {
          const prepared = prepare(commandId);
          if (!prepared.ok) {
            return err(prepared.error.code);
          }
          const result = await setAdminBusinessWorldEnabled(host, prepared.value, signal);
          return result.ok ? ok(result.value) : err(result.error.code);
        } catch {
          // 执行体异常按结果未知处理；异常对象与消息不进入控制台。
          return err("network_error");
        }
      },
      (result) => {
        setIfAlive(
          props.notice,
          { worldId: result.id, enabled: result.enabled, errorCode: null },
          () => true,
        );
      },
      (code) => {
        const refreshRequired = code.startsWith("stale_");
        setIfAlive(
          props.notice,
          { worldId: frozenWorldId, enabled: target, errorCode: code },
          () => true,
        );
        if (refreshRequired) {
          advanceRefresh(props.refresh);
        }
      },
    );
    if (submitted) {
      confirmOpen.set(false);
      props.notice.set(null);
    }
  };

  return (
    <>
      <article class="admin-business-world">
        <div class="admin-business-world__heading">
          <h3>{worldName()}</h3>
          <span
            class={props.world.enabled ? "admin-config-fact-enabled" : "admin-config-fact-disabled"}
          >
            {props.world.enabled
              ? t("admin-business-world-enabled")
              : t("admin-business-world-disabled")}
          </span>
        </div>
        <dl class="admin-business-world__facts">
          <div>
            <dt>{t("admin-business-world-availability")}</dt>
            <dd>{availability()}</dd>
          </div>
          <div>
            <dt>{t("admin-business-world-revision")}</dt>
            <dd>{props.world.policyRevision}</dd>
          </div>
        </dl>
        <button
          class={props.world.enabled ? "admin-btn-danger" : "admin-btn-primary"}
          type="button"
          disabled={blocked()}
          aria-busy={actionBusy() ? "true" : "false"}
          onClick={openConfirm}
        >
          {actionLabel()}
        </button>
      </article>
      <Show when={confirmOpen()}>
        <Dialog
          title={
            props.world.enabled
              ? t("admin-business-world-disable-confirm-title")
              : t("admin-business-world-enable-confirm-title")
          }
          closeLabel={t("admin-users-action-cancel")}
          busy={actionBusy()}
          onClose={() => confirmOpen.set(false)}
        >
          <p class="admin-panel__note">
            <Show
              when={props.world.enabled}
              fallback={t("admin-business-world-enable-confirm-description", {
                world: worldName(),
              })}
            >
              {t("admin-business-world-disable-confirm-description", {
                world: worldName(),
              })}
            </Show>
          </p>
          <div class="admin-shell__dialog-actions">
            <button
              class={props.world.enabled ? "admin-btn-danger" : "admin-btn-primary"}
              type="button"
              disabled={blocked()}
              onClick={submit}
            >
              {actionBusy() ? t("admin-action-working") : t("admin-users-action-confirm")}
            </button>
            <button
              class="admin-btn-secondary"
              type="button"
              disabled={actionBusy()}
              onClick={() => confirmOpen.set(false)}
            >
              {t("admin-users-action-cancel")}
            </button>
          </div>
        </Dialog>
      </Show>
    </>
  );
}

/** 可用性取值的当前语言展示名；未知值按原文显示。 */
function availabilityLabel(availability: string): string {
  switch (availability) {
    case "available":
      return t("admin-business-world-availability-available");
    case "degraded":
      return t("admin-business-world-availability-degraded");
    case "unavailable":
      return t("admin-business-world-availability-unavailable");
    default:
      return availability;
  }
}
