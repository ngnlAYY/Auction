// 当前会话中挂载的读取视图；按实体分发命令了结后的失效通知。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { getOwner, onCleanup } from "solid-js";

import {
  type CommandNotice,
  createPendingCommands,
  type PendingCommandGate,
  type PendingCommandHost,
  type PendingCommands,
  requirePendingCommandHost,
} from ".";

/** 一次页面挂载的读取视图；投影按实体或集合匹配已了结的命令。 */
export interface CommandSource {
  readonly reload: Cell<bigint>;
  readonly gate: PendingCommandGate | null;
  readonly rejections: Cell<readonly CommandNotice[]>;
  readonly matchesEntity: (entity: string) => boolean;
}

/** 一份作用域绑定的存活标记；标记随绑定时的 owner 清理失效。 */
interface LifetimeToken {
  readonly active: () => boolean;
}

/** 读取世代的创建作用域；登记组件只借用世代，不能代表它自己的生命周期。 */
interface ReloadLifetime {
  /** 最近一次显式登记创建作用域得到的标记；快照取得后继续使用它自己那份。 */
  creation: LifetimeToken | null;
  /** 仍在使用该世代的登记组件；只用于没有创建绑定时直接推进的兜底。 */
  readonly registrations: Set<LifetimeToken>;
}

const RELOAD_LIFETIMES = new WeakMap<Cell<bigint>, ReloadLifetime>();
/** 每个登记组件自己的存活标记；旧快照只认取得它的那一次登记。 */
const SOURCE_LIFETIMES = new WeakMap<CommandSource, LifetimeToken>();

/** Rust `u64::MAX`；世代与初值都不得越过这个边界。 */
const U64_MAX = 18_446_744_073_709_551_615n;

function validGeneration(value: bigint): boolean {
  return value >= 0n && value <= U64_MAX;
}

function assertGeneration(value: bigint): void {
  if (!validGeneration(value)) {
    throw new RangeError("读取世代超出 u64 范围");
  }
}

/**
 * 绑定当前作用域的存活标记。
 *
 * 用本次创建的 `onCleanup` 而不是 `isDisposed()`：响应式计算重跑时旧 owner 不一定被标为
 * disposed，但它的 `onCleanup` 会执行。没有作用域的模块级绑定保持永久存活。
 */
function createLifetimeToken(): LifetimeToken {
  let active = true;
  if (getOwner() !== null) {
    onCleanup(() => {
      active = false;
    });
  }
  return { active: () => active };
}

function lifetimeFor(reload: Cell<bigint>): ReloadLifetime {
  let lifetime = RELOAD_LIFETIMES.get(reload);
  if (lifetime === undefined) {
    lifetime = { creation: null, registrations: new Set() };
    RELOAD_LIFETIMES.set(reload, lifetime);
  }
  return lifetime;
}

/**
 * 创建可被未决命令推进的读取世代。
 *
 * 世代随创建它的作用域销毁；运行配置这类由父级创建、子页登记的世代必须在父级
 * 调用本函数（或 {@link trackCommandReload}），否则子页卸载后无法判断世代是否仍存活。
 */
export function createCommandReload(initial: bigint = 0n): Cell<bigint> {
  assertGeneration(initial);
  const reload = createCell(initial);
  trackCommandReload(reload);
  return reload;
}

/**
 * 在创建读取状态的作用域里登记存活判定。
 *
 * 适用于调用方已经用 `createCell` 建好世代、但无法改用 {@link createCommandReload}
 * 的接线。每次调用生成独立的存活标记并成为当前的创建绑定；之前取得的快照继续使用
 * 它自己的标记，不会因为同一世代再次登记而恢复存活。
 */
export function trackCommandReload(reload: Cell<bigint>): void {
  // 既有 Cell 也可能带着调用方写入的越界值；与工厂一样在接线入口拒绝。
  assertGeneration(reload.peek());
  lifetimeFor(reload).creation = createLifetimeToken();
}

/**
 * 世代的创建作用域是否还在；没有显式登记时退回任一仍存活的登记组件。
 *
 * 显式登记优先：创建作用域销毁后，即使还有旧组件登记着这个世代，也不能再推进它。
 */
function reloadAlive(reload: Cell<bigint>): () => boolean {
  const lifetime = RELOAD_LIFETIMES.get(reload);
  if (lifetime === undefined) {
    // 没有生命周期信息的世代保持旧行为；调用方可改用 createCommandReload 精确判定。
    return () => true;
  }
  const creation = lifetime.creation;
  if (creation !== null) {
    return creation.active;
  }
  return () => {
    for (const registration of lifetime.registrations) {
      if (registration.active()) {
        return true;
      }
    }
    return false;
  };
}

/**
 * 快照使用的存活判定。
 *
 * 显式创建绑定按取得快照时的那份标记判定；没有创建绑定时只认这次登记组件自己的标记，
 * 因此同一世代注销后再次登记，旧快照不会恢复存活。
 */
function snapshotAlive(host: PendingCommandHost, source: CommandSource): () => boolean {
  const creation = RELOAD_LIFETIMES.get(source.reload)?.creation ?? null;
  if (creation !== null) {
    return creation.active;
  }
  const registration = SOURCE_LIFETIMES.get(source);
  if (registration !== undefined) {
    return registration.active;
  }
  // 外部构造的来源没有登记标记；只认它自己仍在登记表中。
  return () => host.sources.peek().some((candidate) => candidate.deref() === source);
}

/** 注册当前挂载的读取视图。投影按实体或集合匹配已了结的命令。 */
export function usePendingCommands(
  reload: Cell<bigint>,
  matchesEntity: (entity: string) => boolean,
): PendingCommands {
  return usePendingCommandsWithGate(reload, null, matchesEntity);
}

/**
 * 页级门禁随本次挂载结束；当前页面的重读订阅由实体匹配决定。
 *
 * 源码在这里用 `use_context_provider` 把所有者提供给子树；Solid 的 hook 不能替调用方包 JSX，
 * 因此返回的所有者须由调用组件用 `<PendingCommandsContext value={owner}>` 包住自己的 JSX。
 * 世代由父级创建、子页登记时，父级应使用 {@link createCommandReload} 或
 * {@link trackCommandReload}，让宿主能判断父级作用域是否仍存活。
 */
export function usePendingCommandsWithGate(
  reload: Cell<bigint>,
  gate: PendingCommandGate | null,
  matchesEntity: (entity: string) => boolean,
): PendingCommands {
  const host = requirePendingCommandHost();
  const rejections = createCell<readonly CommandNotice[]>([]);
  const source = createCell<CommandSource>({
    reload,
    gate,
    rejections,
    matchesEntity,
  });
  const registered = source.peek();
  const registration = new WeakRef(registered);
  // 登记组件与世代创建作用域可能不同：运行配置的世代由认证外壳创建，页面只登记。
  // 这次登记产生独立的存活标记，快照持有标记本身；注销后旧快照不能靠重新登记恢复。
  const registrationAlive = createLifetimeToken();
  SOURCE_LIFETIMES.set(registered, registrationAlive);
  const lifetime = lifetimeFor(reload);
  lifetime.registrations.add(registrationAlive);
  host.sources.update((sources) => [...sources, registration]);
  onCleanup(() => {
    // 显式注销让外壳立即重新计算重试和回执入口。同一会话的在途请求仍由宿主持有。
    lifetime.registrations.delete(registrationAlive);
    host.sources.update((sources) =>
      sources.filter((candidate) => candidate.deref() !== registered),
    );
  });
  return createPendingCommands(host, source);
}

/** 在登记表中重新取得仍存活于会话的读取视图。 */
export function liveSource(
  host: PendingCommandHost,
  source: WeakRef<CommandSource>,
): CommandSource | null {
  const target = source.deref();
  if (target === undefined) {
    return null;
  }
  // 响应式读取登记表：来源页卸载时立即通知重试入口与 Key 回执重新计算。
  const mounted = host.sources().find((candidate) => candidate.deref() === target);
  return mounted === undefined ? null : target;
}

/** 本次了结需要推进的读取世代；每个匹配且仍挂载的视图各一份，并携带创建作用域的存活判定。 */
export function reloadsFor(
  host: PendingCommandHost,
  entity: string,
): readonly {
  readonly reload: Cell<bigint>;
  readonly previous: bigint;
  readonly alive: () => boolean;
}[] {
  const reloads: {
    readonly reload: Cell<bigint>;
    readonly previous: bigint;
    readonly alive: () => boolean;
  }[] = [];
  for (const registration of host.sources.peek()) {
    const source = registration.deref();
    if (source === undefined || !source.matchesEntity(entity)) {
      continue;
    }
    const alive = snapshotAlive(host, source);
    // 源码的 try_peek 在世代信号已销毁时跳过本次刷新；采样时就排除已失效的世代。
    if (!alive()) {
      continue;
    }
    const previous = source.reload.peek();
    // 调用方可能在同一 Cell 里写入越界值；与推进处一样拒绝超出 u64 的世代。
    if (!validGeneration(previous)) {
      continue;
    }
    reloads.push({ reload: source.reload, previous, alive });
  }
  return reloads;
}

/** 草稿清理和当前读取世代一起推进，防止较早的读取结果重新装配旧数据。 */
export function advanceCommandReload(reload: Cell<bigint>): void {
  // 源码的 try_write 在信号已销毁时静默跳过。创建作用域已销毁的世代不能再被写入，
  // 即使命令交付后仍持有旧 Cell；父级创建、子页登记的世代由创建的父级判定存活。
  if (!reloadAlive(reload)()) {
    return;
  }
  // 共享 Cell 没有 `try_write`；通过存活判定后写入总是生效，等价于源码在可写时推进世代的分支。
  // 用 bigint 保留 Rust `u64` 的边界，并与源码一样在 `u64::MAX` 饱和。
  const current = reload.peek();
  // 调用方可能在采样后写入越界值；越界世代不再推进。
  if (!validGeneration(current)) {
    return;
  }
  reload.set(current === U64_MAX ? current : current + 1n);
}
