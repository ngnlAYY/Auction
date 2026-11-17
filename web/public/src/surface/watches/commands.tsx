//! 标记命令属于页面的稳定用户宿主；目录 GET、阶段切换和分页不拥有提交任务。

import type { PublicSessionCurrentResponse } from "@auction/contracts/dto/public/account";
import type { PublicSetLotWatchRequest } from "@auction/contracts/dto/public/watch";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { TaskScope } from "@auction/web-shared/reactive/tasks";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import type { Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import { setContext } from "@solidjs/signals";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, For, Show, useContext } from "solid-js";
import type { CommandError } from "../../api/commands";
import { commandErrorOutcomeUncertain } from "../../api/commands";
import { setLotWatch } from "../../api/watches";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
} from "..";

/** 同一 lot 的冻结命令；成功后等待整场集合重读的修订号。 */
export interface WatchCommand {
  readonly auctionId: string;
  /** 拍品编号只用于页面级状态条辨认装备，留在宿主内存，不进入请求。 */
  readonly lotCode: string;
  readonly payload: PublicSetLotWatchRequest;
  readonly busy: boolean;
  /** 同一命令的未知结果累计保留；后续前置拒绝不能证明早先请求未提交。 */
  readonly uncertain: boolean;
  readonly error: CommandError | null;
  /** 成功后等待的标记修订号；读到包含本次结果的集合前锁住该 lot，不用旧版本发起 CAS。 */
  readonly awaitingRefresh: number | null;
}

export type WatchSubmit = (
  auctionId: string,
  lotId: string,
  lotCode: string,
  payload: PublicSetLotWatchRequest,
) => void;

/** 标记作用域实际需要的命令宿主部分。 */
export interface WatchCommandStore {
  readonly revision: Cell<number>;
  readonly commands: Cell<ReadonlyMap<string, WatchCommand>>;
  readonly submit: WatchSubmit;
}

export interface WatchCommandOwner extends WatchCommandStore {
  readonly userId: string | null;
  readonly identity: Cell<string | null>;
  readonly session: PublicSessionResource;
  readonly tasks: TaskScope;
}

const detachedCommands = createCell<ReadonlyMap<string, WatchCommand>>(new Map());
const detachedRevision = createCell(0);
const detachedStore: WatchCommandStore = {
  revision: detachedRevision,
  commands: detachedCommands,
  submit: () => {},
};

/** 稳定身份宿主渲染的子树与状态条读取同一份命令状态；缺省允许只读行按无宿主处理。 */
const WatchCommandOwnerContext = createContext<WatchCommandOwner | null>(null);

/** 标记作用域读取的宿主；没有命令宿主时使用空操作，避免破坏只读渲染。 */
export function watchCommandStore(): WatchCommandStore {
  return useContext(WatchCommandOwnerContext) ?? detachedStore;
}

/** 已认证分支必须处于命令宿主内；缺少宿主是装配错误。 */
export function requireWatchCommandOwner(): WatchCommandOwner {
  const owner = useContext(WatchCommandOwnerContext);
  if (owner === null) {
    throw new Error("LotWatchButton 必须在标记命令宿主内渲染");
  }
  return owner;
}

/** 由已按用户 keyed 的页面宿主无条件调用；集合只有一个 Cell，不为每次命令分配 Cell。 */
export function useWatchCommandOwner(userId: string | null): JSX.Element {
  const revision = createCell(0);
  const identitySource = useContext(PublicCommandOwnerContext);
  // 上下文只暴露访问器；事件处理器需要 peek 读取身份，因此在本宿主保存一份可读值。
  const identity = createCell(identitySource());
  createEffect(
    () => identitySource(),
    (current) => {
      identity.set(current);
    },
  );
  const session = useContext(PublicSessionResourceContext);
  const commands = createCell<ReadonlyMap<string, WatchCommand>>(new Map());
  const tasks = createTaskScope();
  const owner: WatchCommandOwner = {
    userId,
    identity,
    session,
    revision,
    commands,
    tasks,
    submit: (auctionId, lotId, lotCode, payload) =>
      submitCommand(owner, auctionId, lotId, lotCode, payload),
  };
  // 宿主把子树与状态条作为兄弟渲染，调用方无法再把二者一起包进 Provider；在调用组件
  // 自己的 owner 上写入上下文，既保留子树隔离，也让新增子树无需经过模块级登记。
  setContext(WatchCommandOwnerContext, owner);

  // 忙碌反馈由行内按钮承担；页面级状态条只列结果未知、需要原样重试的命令（含重试中的），
  // 即使所在行已卸载也能找到。
  const pending = createValueMemo(() => {
    const entries: { readonly lotId: string; readonly command: WatchCommand }[] = [];
    for (const [lotId, command] of commands()) {
      if (command.uncertain) {
        entries.push({ lotId, command });
      }
    }
    entries.sort((left, right) => compareText(left.lotId, right.lotId));
    return entries;
  });

  return (
    <WatchCommandOwnerContext value={owner}>
      <For each={pending()} keyed={(entry) => entry.lotId}>
        {(entry) => {
          const command = () => entry().command;
          return (
            <div class="public-lot-watches__sync" role="status">
              <p>{command().lotCode}</p>
              <Show
                when={command().busy}
                fallback={
                  <>
                    <p>{t("watch-unknown")}</p>
                    <button
                      type="button"
                      onClick={() =>
                        owner.submit(
                          command().auctionId,
                          entry().lotId,
                          command().lotCode,
                          command().payload,
                        )
                      }
                    >
                      {t("watch-retry-command")}
                    </button>
                  </>
                }
              >
                <p>{t("watch-saving")}</p>
              </Show>
            </div>
          );
        }}
      </For>
    </WatchCommandOwnerContext>
  );
}

function submitCommand(
  owner: WatchCommandOwner,
  auctionId: string,
  lotId: string,
  lotCode: string,
  payload: PublicSetLotWatchRequest,
): void {
  if (!ownerIdentityMatches(owner)) {
    return;
  }
  const userId = owner.userId;
  if (userId === null) {
    return;
  }
  const current = owner.commands.peek();
  let frozen: PublicSetLotWatchRequest;
  let uncertain: boolean;
  const pending = current.get(lotId);
  if (pending !== undefined) {
    if (
      pending.busy ||
      !pending.uncertain ||
      pending.auctionId !== auctionId ||
      pending.payload.command_id !== payload.command_id
    ) {
      return;
    }
    // 不接受调用方替换版本或新意图。
    frozen = structuredClone(pending.payload);
    uncertain = pending.uncertain;
  } else {
    if (owner.session.state() !== "ready" || currentUser(owner.session()) !== userId) {
      return;
    }
    frozen = structuredClone(payload);
    uncertain = false;
  }
  const next = new Map(current);
  next.set(lotId, {
    auctionId,
    lotCode,
    payload: structuredClone(frozen),
    busy: true,
    uncertain,
    error: null,
    awaitingRefresh: null,
  });
  owner.commands.set(next);
  owner.tasks.spawn(async (context) => {
    const result = await setLotWatch(userId, auctionId, lotId, frozen, context.signal);
    if (!context.alive() || !ownerIdentityMatches(owner)) {
      return;
    }
    if (result.ok) {
      // 命令重放可能返回旧版本；始终重读集合，绝不直接覆盖权威状态。
      // 重读完成前保留条目锁住该 lot，界面先显示本次提交的目标状态。只推进标记
      // 修订：当前场次、目录与竞价记录不含标记状态，不随之重读。
      const revision = saturatingAddOne(owner.revision.peek());
      const commands = new Map(owner.commands.peek());
      commands.set(lotId, {
        auctionId,
        lotCode,
        payload: structuredClone(frozen),
        busy: false,
        uncertain: false,
        error: null,
        awaitingRefresh: revision,
      });
      owner.commands.set(commands);
      owner.revision.set(revision);
      return;
    }
    const error = result.error;
    // 只有路由的明确冲突码能解除未知；裸 409 或损坏响应仍须原样重试。
    const nextUncertain =
      !(error.status === 409 && error.code === "watch_conflict") &&
      (uncertain || commandErrorOutcomeUncertain(error));
    const commands = new Map(owner.commands.peek());
    commands.set(lotId, {
      auctionId,
      lotCode,
      payload: structuredClone(frozen),
      busy: false,
      uncertain: nextUncertain,
      error,
      awaitingRefresh: null,
    });
    owner.commands.set(commands);
  });
}

function ownerIdentityMatches(owner: WatchCommandOwner): boolean {
  return owner.userId !== null && owner.identity.peek() === owner.userId;
}

export function currentUser(
  snapshot: Result<PublicSessionCurrentResponse | null, unknown> | undefined,
): string | null {
  if (snapshot === undefined || !snapshot.ok || snapshot.value === null) {
    return null;
  }
  return snapshot.value.authenticated ? snapshot.value.user_id : null;
}

function saturatingAddOne(value: number): number {
  return value === Number.MAX_SAFE_INTEGER ? value : value + 1;
}
