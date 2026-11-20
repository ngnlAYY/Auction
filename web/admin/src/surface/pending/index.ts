// 认证会话内持有已提交命令，按实体限制重复意图并通知当前显示该实体的读取视图。

import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope, type TaskScope } from "@auction/web-shared/reactive/tasks";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createContext, onCleanup, useContext } from "solid-js";

import { type AdminWriteFailure, writeOutcomeUnknown } from "../../api/failure";
import { type AdminSessionHost, adminSessionWritesAllowed } from "../../api/session";
import type { AdminUserGeneratedKey } from "../users/create";
import {
  type CommandDeliver,
  completeCommand,
  copyCommandReceipt,
  freezeNotices,
} from "./completion";
import { type CommandSource, liveSource } from "./sources";

/** 一次尝试的异步结果；成功时携带交付回调。 */
type CommandAttempt = Promise<Result<CommandDeliver, AdminWriteFailure>>;

/** 冻结的命令工厂；重试只复用首次确认的 UUID、负载与成功回调。 */
type CommandFactory = (commandId: string, signal: AbortSignal) => CommandAttempt;

/**
 * 页面提供的命令执行体；取消信号由宿主的任务上下文提供。
 * 页面在提交时把输入冻结成不可变值；执行体用 `prepareJsonCommandOnce` 在第一次运行时
 * 生成并缓存正文，之后每次用 `sendPreparedJsonWrite` 复用同一份。执行体不得读取草稿
 * 信号或重新序列化。
 */
export type CommandOperation<T> = (
  commandId: string,
  signal: AbortSignal,
) => Promise<Result<T, AdminWriteFailure>>;

/** 会话实体上的稳定命令标识；同一实体的未了结命令阻止新意图。 */
export interface CommandIdentity {
  readonly entity: string;
  readonly action: string;
  readonly commandId: string;
}

/** 当前会话登记的一条已提交（或结果未知）的命令。 */
export interface PendingCommand {
  readonly identity: CommandIdentity;
  readonly source: WeakRef<CommandSource>;
  readonly busy: boolean;
  readonly unknown: boolean;
  readonly error: string | null;
  readonly factory: CommandFactory;
  readonly onFailure: (code: string) => void;
}

/** 已了结但仍需回显的命令结果；来源页卸载后由会话继续持有。 */
export interface CommandNotice {
  readonly identity: CommandIdentity;
  readonly result: Result<CommandReceipt | null, string>;
  readonly keySource: WeakRef<CommandSource> | null;
}

/** 来源页卸载后仍需交付的安全回执信息；Key 只能由操作员打开一次性对话框查看。 */
export type CommandReceipt =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "generated-key"; readonly key: AdminUserGeneratedKey };

/** 成功回调确认页面是否实际显示结果。仍需交付的内容由会话持有。 */
export type CommandDelivery =
  | { readonly kind: "delivered" }
  | { readonly kind: "receipt"; readonly receipt: CommandReceipt | null };

/** 按源码 `CommandDelivery::reported` 的语义，根据来源页是否展示结果决定回执。 */
export function commandDeliveryReported(
  delivered: boolean,
  receipt: () => CommandReceipt | null,
): CommandDelivery {
  return delivered ? { kind: "delivered" } : { kind: "receipt", receipt: receipt() };
}

const EMPTY_PENDING_COMMANDS: readonly PendingCommand[] = Object.freeze([]);
const EMPTY_COMMAND_NOTICES: readonly CommandNotice[] = Object.freeze([]);
const EMPTY_COMMAND_SOURCES: readonly WeakRef<CommandSource>[] = Object.freeze([]);

function sourceRegistered(host: PendingCommandHost, source: CommandSource): boolean {
  // 共享 Cell 没有 `try_peek`；来源页注销与组件销毁同步，以注册表判断是否仍可提交。
  return host.sources.peek().some((registration) => registration.deref() === source);
}

/** 唯一的会话宿主；条目与回执只驻留在当前认证外壳。 */
export interface PendingCommandHost {
  readonly entries: Cell<readonly PendingCommand[]>;
  readonly notices: Cell<readonly CommandNotice[]>;
  readonly sources: Cell<readonly WeakRef<CommandSource>[]>;
  /** 认证外壳登记时发给本次挂载的会话句柄；命令入口只读这一份许可。 */
  readonly sessionHost: AdminSessionHost;
  /** 宿主的认证作用域仍存活；销毁后命令入口不再写入条目、回执或来源登记。 */
  alive(): boolean;
  /** 认证外壳的任务作用域，对应源码在宿主 Scope 内 spawn：站内导航不取消已发出的命令，
   * 认证子树销毁时同时释放请求、冻结负载和回执。 */
  readonly tasks: TaskScope;
  retry(commandId: string): void;
  abandon(commandId: string): boolean;
  dismiss(commandId: string): boolean;
}

/** 认证外壳安装一次的会话命令宿主；缺少提供方是装配错误。 */
export const PendingCommandHostContext = createContext<PendingCommandHost | null>(null);

/** 读取当前认证外壳的命令宿主。 */
export function requirePendingCommandHost(): PendingCommandHost {
  const host = useContext(PendingCommandHostContext);
  if (host === null) {
    throw new Error("未决命令宿主缺失");
  }
  return host;
}

/** 供来源页与它的子组件查询会话实体占用，并标记回执的交付位置。 */
export interface PendingCommands {
  readonly host: PendingCommandHost;
  readonly source: Cell<CommandSource>;
  /** 同实体的发送中或未知记录跨页面阻止新意图。 */
  contains(entity: string): boolean;
  /** 区分正在发送与已停止但未决；浏览筛选可在请求结束后恢复，实体的新意图仍被封锁。 */
  isBusy(entity: string): boolean;
  /** 保留本页面的串行操作策略，其他来源的请求只占用自己的实体。 */
  anyBusy(): boolean;
  /** 本来源页的同类操作仍可共用同一个页内串行门禁。 */
  containsLocal(prefix: string): boolean;
  /** 只登记已经确认的命令。此入口的成功反馈由来源页面自身持有。 */
  submit<T>(
    entity: string,
    action: string,
    operation: CommandOperation<T>,
    onSuccess: (value: T) => void,
    onFailure: (code: string) => void,
  ): boolean;
  /** 投影成功回执。仅在 `sourceAlive` 时访问来源页面，临时行或卡片仍须检查自己的存活。
   * 成功回调应先构造回执，再做可能失败的页面反馈；宿主会隔离回调异常，但抛错前尚未
   * 构造的回执无法补交。 */
  submitWithReceipt<T>(
    entity: string,
    action: string,
    operation: CommandOperation<T>,
    onSuccess: (value: T, sourceAlive: boolean) => CommandDelivery,
    onFailure: (code: string) => void,
  ): boolean;
  /** 一次性 Key 由当前页面展示，回执由会话持有，直到操作员确认。 */
  generatedKey(): { readonly commandId: string; readonly key: AdminUserGeneratedKey } | null;
  acknowledgeKey(commandId: string): void;
  dismiss(commandId: string): void;
}

/** 来源页及其子组件读取同一份会话命令状态；缺少提供方是装配错误。 */
export const PendingCommandsContext = createContext<PendingCommands | null>(null);

/** 读取当前来源页的命令所有者。 */
export function requirePendingCommands(): PendingCommands {
  const owner = useContext(PendingCommandsContext);
  if (owner === null) {
    throw new Error("未决命令来源缺少所有者");
  }
  return owner;
}

/** 可选页级串行门禁，只保存状态；一次性结果由页面展示，不进入此处。 */
export interface PendingCommandGate {
  /** 供控件展示及入口守卫读取；实际执行仍须同步取得占用。 */
  blocked(): boolean;
  /** 必须在 spawn 前同步检查并占用，不能用渲染或 effect 作为执行授权。 */
  tryAcquire(): PendingCommandGateGuard | null;
  /** 成功结果已交页面；直到操作员确认展示完成前继续封锁新请求。 */
  holdResult(): void;
  /** 只确认已展示结果，不允许关闭动作误释放仍在运行的请求。 */
  acknowledge(): void;
}

/** 请求结束或取消时释放运行占用，但不能解除页面待确认的一次性结果。 */
export interface PendingCommandGateGuard {
  release(): void;
}

type CommandGateState = "ready" | "running" | "awaiting-acknowledgement";

/** 由稳定页面创建，与首次提交及未决命令重试共享。 */
export function usePendingCommandGate(): PendingCommandGate {
  const state = createCell<CommandGateState>("ready");
  return {
    blocked: () => state() !== "ready",
    tryAcquire: () => {
      if (state.peek() !== "ready") {
        return null;
      }
      state.set("running");
      let released = false;
      return {
        release: () => {
          if (released) {
            return;
          }
          released = true;
          // 页面销毁也可能释放信号；复位只影响已卸载页面的状态，不需要再检查宿主是否存活。
          if (state.peek() === "running") {
            state.set("ready");
          }
        },
      };
    },
    holdResult: () => state.set("awaiting-acknowledgement"),
    acknowledge: () => {
      if (state.peek() === "awaiting-acknowledgement") {
        state.set("ready");
      }
    },
  };
}

/** 由认证外壳安装一次；每个请求都由这个 Scope 持有。 */
export function usePendingCommandHost(sessionHost: AdminSessionHost): PendingCommandHost {
  const entries = createCell<readonly PendingCommand[]>(EMPTY_PENDING_COMMANDS);
  const notices = createCell<readonly CommandNotice[]>(EMPTY_COMMAND_NOTICES);
  const sources = createCell<readonly WeakRef<CommandSource>[]>(EMPTY_COMMAND_SOURCES);
  const tasks = createTaskScope();
  let alive = true;
  const host: PendingCommandHost = {
    entries,
    notices,
    sources,
    sessionHost,
    alive: () => alive,
    tasks,
    retry: (commandId) => {
      retryPendingCommand(host, commandId);
    },
    abandon: (commandId) => abandonPendingCommand(host, commandId),
    dismiss: (commandId) => dismissCommandNotice(host, commandId),
  };
  onCleanup(() => {
    // 对应源码信号随认证作用域销毁：旧句柄立即失去负载与回执，之后的命令入口也不再写入。
    alive = false;
    entries.set(EMPTY_PENDING_COMMANDS);
    notices.set(EMPTY_COMMAND_NOTICES);
    sources.set(EMPTY_COMMAND_SOURCES);
  });
  return host;
}

/** 来源页注册实体匹配与页级门禁；对外暴露 {@link PendingCommands} 的完整方法面。 */
export function createPendingCommands(
  host: PendingCommandHost,
  source: Cell<CommandSource>,
): PendingCommands {
  const owner: PendingCommands = {
    host,
    source,
    contains: (entity) => host.entries().some((entry) => entry.identity.entity === entity),
    isBusy: (entity) =>
      host.entries().some((entry) => entry.identity.entity === entity && entry.busy),
    anyBusy: () => {
      const currentSource = source();
      return host
        .entries()
        .some((entry) => entry.busy && pendingCommandBelongsTo(entry, currentSource));
    },
    containsLocal: (prefix) => {
      const currentSource = source();
      return host
        .entries()
        .some(
          (entry) =>
            entry.identity.entity.startsWith(prefix) &&
            pendingCommandBelongsTo(entry, currentSource),
        );
    },
    submit: (entity, action, operation, onSuccess, onFailure) =>
      owner.submitWithReceipt(
        entity,
        action,
        operation,
        (result, sourceAlive) => {
          if (sourceAlive) {
            try {
              onSuccess(result);
            } catch {
              // 页面反馈异常不能改变已经成功的命令结果；回执仍按已交付处理。
            }
            return { kind: "delivered" };
          }
          return { kind: "receipt", receipt: null };
        },
        onFailure,
      ),
    submitWithReceipt: (entity, action, operation, onSuccess, onFailure) => {
      const currentSource = source.peek();
      if (
        !host.alive() ||
        !sourceRegistered(host, currentSource) ||
        !adminSessionWritesAllowed(host.sessionHost) ||
        owner.contains(entity)
      ) {
        return false;
      }
      if (currentSource.gate?.blocked() === true) {
        return false;
      }
      // 自定义门禁也能在 blocked() 里同步销毁宿主、注销来源或重入提交同一实体；
      // 登记条目之前按同一组条件重新核对。
      if (
        !host.alive() ||
        !sourceRegistered(host, currentSource) ||
        !adminSessionWritesAllowed(host.sessionHost) ||
        owner.contains(entity)
      ) {
        return false;
      }
      const commandId = createUuidV4();
      // 重试只使用首次确认的工厂和 UUID，不再读取当前草稿或版本。
      const factory: CommandFactory = async (id, signal) => {
        const result = await operation(id, signal);
        return result.ok
          ? ok((sourceAlive: boolean) => onSuccess(result.value, sourceAlive))
          : result;
      };
      currentSource.rejections.update((current) =>
        freezeNotices(current.filter((notice) => notice.identity.entity !== entity)),
      );
      host.entries.update((current) => [
        ...current,
        {
          identity: Object.freeze({ entity, action, commandId }),
          source: new WeakRef(currentSource),
          busy: false,
          unknown: false,
          error: null,
          factory,
          onFailure,
        },
      ]);
      if (retryPendingCommand(host, commandId) !== "started") {
        // 门禁可能在取得 guard 前返回 null、同步销毁宿主或移除条目；首次提交未启动时
        // 不能留下一条既不忙也不未知的记录把同一实体锁死。
        if (host.alive()) {
          const pending = host.entries
            .peek()
            .find((candidate) => candidate.identity.commandId === commandId);
          if (pending === undefined || !pending.busy) {
            host.entries.update((current) =>
              current.filter((candidate) => candidate.identity.commandId !== commandId),
            );
          }
        }
        return false;
      }
      return true;
    },
    generatedKey: () => {
      if (!host.alive()) {
        return null;
      }
      const currentSource = source();
      for (const notice of host.notices()) {
        // 旧来源注销后登记表不再返回它；只比较对象身份会让旧句柄继续读到 Key。
        const presenting = notice.keySource === null ? null : liveSource(host, notice.keySource);
        if (presenting !== currentSource) {
          continue;
        }
        if (
          notice.result.ok &&
          notice.result.value !== null &&
          notice.result.value.kind === "generated-key"
        ) {
          const receipt = copyCommandReceipt(notice.result.value);
          if (receipt.kind !== "generated-key") {
            continue;
          }
          return Object.freeze({
            commandId: notice.identity.commandId,
            key: receipt.key,
          });
        }
      }
      return null;
    },
    acknowledgeKey: (commandId) => host.dismiss(commandId),
    dismiss: (commandId) => {
      if (!host.alive()) {
        return;
      }
      source
        .peek()
        .rejections.update((current) =>
          freezeNotices(current.filter((notice) => notice.identity.commandId !== commandId)),
        );
    },
  };
  return owner;
}

/** 条目是否属于给定来源页；来源页卸载后注册表不再返回它。 */
export function pendingCommandBelongsTo(entry: PendingCommand, source: CommandSource): boolean {
  return entry.source.deref() === source;
}

/** 重试入口是否被忙碌状态或存活来源页的串行门禁阻止。 */
export function pendingCommandRetryBlocked(
  entry: PendingCommand,
  host: PendingCommandHost,
): boolean {
  const source = liveSource(host, entry.source);
  return entry.busy || source?.gate?.blocked() === true;
}

type CommandStart = "started" | "blocked" | "invalid";

function retryPendingCommand(host: PendingCommandHost, commandId: string): CommandStart {
  if (!host.alive() || !adminSessionWritesAllowed(host.sessionHost)) {
    return "blocked";
  }
  const current = host.entries.peek();
  const index = current.findIndex((entry) => entry.identity.commandId === commandId);
  const entry = index < 0 ? undefined : current[index];
  if (entry === undefined || entry.busy) {
    return "blocked";
  }
  const source = liveSource(host, entry.source);
  const gate = source?.gate ?? null;
  const guard = gate === null ? null : gate.tryAcquire();
  // 自定义门禁可以在 tryAcquire() 里同步销毁宿主或改动条目；即使返回 null 也要先复核。
  if (!host.alive()) {
    guard?.release();
    return "invalid";
  }
  const afterGate = host.entries.peek();
  const afterIndex = afterGate.findIndex((candidate) => candidate.identity.commandId === commandId);
  const afterEntry = afterIndex < 0 ? undefined : afterGate[afterIndex];
  if (afterEntry !== entry || afterEntry.busy) {
    guard?.release();
    return "invalid";
  }
  if (gate !== null && guard === null) {
    return "blocked";
  }
  host.entries.set(
    afterGate.map((candidate, candidateIndex) =>
      candidateIndex === afterIndex ? { ...candidate, busy: true } : candidate,
    ),
  );
  const factory = entry.factory;
  let released = false;
  const releaseGuard = (): void => {
    if (released) {
      return;
    }
    released = true;
    guard?.release();
  };
  const handle = host.tasks.spawn(async (context) => {
    // 取消边界就是 Rust guard 被丢弃的时机：signal 中止时立即释放门禁，不再等待 factory 结束。
    context.signal.addEventListener("abort", releaseGuard, { once: true });
    try {
      // Dioxus 的 spawn 先把任务入队；让出一次微任务，保留首次发送前取消的窗口。
      await Promise.resolve();
      if (!context.alive()) {
        return;
      }
      let result: Result<CommandDeliver, AdminWriteFailure>;
      try {
        result = await factory(commandId, context.signal);
      } catch {
        if (!context.alive()) {
          return;
        }
        // 操作接口用 Result 表达完成结果；异常拒绝是程序错误，不能当成明确拒绝。
        // 异常原文可能含秘密，不进入日志；保留原 ID 与工厂，按结果未知收尾，
        // 让操作员可以原样重试或放弃，页面仍保持就绪。
        completeCommand(host, commandId, err(writeOutcomeUnknown("network_error")));
        return;
      }
      if (!context.alive()) {
        return;
      }
      completeCommand(host, commandId, result);
    } finally {
      context.signal.removeEventListener("abort", releaseGuard);
      releaseGuard();
    }
  });
  // 宿主已销毁时 spawn 不执行任务体，finally 不会被调用；这里补一次释放。
  if (handle.finished()) {
    releaseGuard();
  }
  return "started";
}

function abandonPendingCommand(host: PendingCommandHost, commandId: string): boolean {
  if (!host.alive()) {
    return false;
  }
  // 放弃只释放本地记录和负载。服务器仍可能执行已发出的命令。
  const current = host.entries.peek();
  const index = current.findIndex(
    (entry) => entry.identity.commandId === commandId && !entry.busy && entry.unknown,
  );
  if (index < 0) {
    return false;
  }
  host.entries.set(current.filter((_, candidateIndex) => candidateIndex !== index));
  return true;
}

function dismissCommandNotice(host: PendingCommandHost, commandId: string): boolean {
  if (!host.alive()) {
    return false;
  }
  const current = host.notices.peek();
  const index = current.findIndex((notice) => notice.identity.commandId === commandId);
  const notice = index < 0 ? undefined : current[index];
  if (notice === undefined) {
    return false;
  }
  host.notices.set(freezeNotices(current.filter((_, candidateIndex) => candidateIndex !== index)));
  const source = notice.keySource === null ? null : liveSource(host, notice.keySource);
  if (source !== null && source.gate !== null) {
    source.gate.acknowledge();
  }
  return true;
}

/** 页面命令完成后只回写仍存活的行状态，旧 Signal 不会指向新挂载的实体。 */
export function setIfAlive<T>(signal: Cell<T>, value: T, alive: () => boolean): void {
  trySetIfAlive(signal, value, alive);
}

/** 反馈控件提前卸载时，调用方用返回值决定是否由外壳补交回执。
 * 共享 `Cell` 没有 `try_write`；存活判定必须来自反馈控件自身的 owner（卸载时置为 false），
 * 不能借用命令任务的 `alive()`，否则临时行或卡片已卸载时仍会写进旧状态。 */
export function trySetIfAlive<T>(signal: Cell<T>, value: T, alive: () => boolean): boolean {
  if (!alive()) {
    return false;
  }
  signal.set(value);
  return true;
}
