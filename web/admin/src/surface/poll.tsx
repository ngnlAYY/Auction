// Admin 运维页面的可选自动轮询：开关与间隔状态由页面持有（入 URL query），本模块提供
// 轮询调度 hook 与开关控件；可见性感知与分片睡眠复用共享层。
// 每个页面拥有独立的可见性监听，卸载时释放回调；开关和任务替换不改变其它页面的状态。

import {
  createPageVisibility,
  type PageVisibility,
  sleepWhileVisible,
} from "@auction/web-shared/platform/visibility";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope, type TaskHandle } from "@auction/web-shared/reactive/tasks";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, onSettled } from "solid-js";

/** 可选的自动轮询间隔（秒）；0 表示关闭。与 URL query `poll` 键的合法值一一对应。 */
export const POLL_INTERVAL_VALUES: readonly number[] = [10, 30, 60];

const UINT64_MAX = 18_446_744_073_709_551_615n;

/** `sleep_while_visible` 的延时上限：源码把毫秒数收窄到 u32，超出按 u32::MAX 处理。 */
const MAX_POLL_DELAY_MILLIS_BIGINT = 4_294_967_295n;

/** 间隔秒数换算成一次等待的毫秒数；秒数超出 u32 毫秒范围时截到上限。 */
function pollDelayMillis(seconds: bigint): number {
  const millis = seconds * 1_000n;
  return Number(millis > MAX_POLL_DELAY_MILLIS_BIGINT ? MAX_POLL_DELAY_MILLIS_BIGINT : millis);
}

/** 可选自动轮询 hook：间隔为 0 时关闭；开启后每隔 `intervalSeconds` 秒执行一次
 * `onPoll`。页面隐藏期间计时挂起而非继续打请求，恢复可见后走完剩余分片继续轮询。
 * 间隔变化（含关闭）时先取消旧任务，再按新间隔从头计时；卸载仍由页面任务作用域取消任务。
 * 取消只释放失效循环的等待与捕获，不补发轮询，也不改变当前请求或页面可见性策略。 */
export function useAutoPoll(intervalSeconds: () => bigint, onPoll: () => void): void {
  const visibility: PageVisibility = createPageVisibility();
  // 只安装一次可见性监听；卸载时释放回调，避免路由反复挂载时监听器累积。
  onSettled(() => {
    visibility.install(() => undefined);
    return () => visibility.cleanup();
  });
  const tasks = createTaskScope();
  // 任务不放入响应式状态；替换任务不应触发组件重渲染或再次执行本 effect。
  let pollTask: TaskHandle | null = null;
  // 源码收到的是按值去重的 `Memo<u64>`；等值通知不重建任务，这里保持同一语义。
  const interval = createValueMemo(intervalSeconds);
  createEffect(interval, (seconds) => {
    pollTask?.cancel();
    pollTask = null;
    if (seconds === 0n) {
      return;
    }
    const delayMillis = pollDelayMillis(seconds);
    pollTask = tasks.spawn(async (context) => {
      for (;;) {
        await sleepWhileVisible(visibility, delayMillis, context.signal);
        if (!context.alive()) {
          return;
        }
        onPoll();
      }
    });
  });
}

/** 下拉值解析：`u64` 的文本形式只允许十进制数字（可带前导 `+`），完整接受 `u64` 范围。 */
function parsePollSeconds(value: string): bigint | null {
  const digits = value.startsWith("+") ? value.slice(1) : value;
  if (digits.length === 0) {
    return null;
  }
  let seconds = 0n;
  for (let index = 0; index < digits.length; index += 1) {
    const code = digits.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
    seconds = seconds * 10n + BigInt(code - 0x30);
    if (seconds > UINT64_MAX) {
      return null;
    }
  }
  return seconds;
}

/** 自动轮询开关控件：单个下拉，可选关闭与 10/30/60 秒；默认值由页面 query 决定
 * （仪表盘默认 30 秒自动轮询），变更由页面写回 URL query。 */
export function AutoPollControls(props: {
  readonly pollSeconds: bigint;
  readonly onChange: (seconds: bigint) => void;
}): JSX.Element {
  return (
    <label class="admin-poll-control">
      {t("admin-poll-label")}
      <select
        value={props.pollSeconds.toString()}
        onChange={(event) => {
          // 下拉只提供合法闭集；解析失败保持现状。
          const seconds = parsePollSeconds(event.currentTarget.value);
          if (seconds !== null) {
            props.onChange(seconds);
          }
        }}
      >
        <option value="0">{t("admin-poll-off")}</option>
        <option value="10">{t("admin-poll-interval", { seconds: 10 })}</option>
        <option value="30">{t("admin-poll-interval", { seconds: 30 })}</option>
        <option value="60">{t("admin-poll-interval", { seconds: 60 })}</option>
      </select>
    </label>
  );
}
