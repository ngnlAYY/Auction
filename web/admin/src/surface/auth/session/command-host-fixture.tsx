// 真实认证布局下的命令页与失败、在途请求替身。

import { ThemeStateContext, useThemeEffects } from "@auction/web-shared/platform/theme";
import { err, type Result } from "@auction/web-shared/result";
import { pageComponent, type Router, RouterContext, useRouter } from "@auction/web-shared/router";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import type { JSX } from "@solidjs/web";
import { createContext, onCleanup, useContext } from "solid-js";
import { type AdminApiFailure, adminApiFailureFromHttpBody } from "../../../api/failure";
import { type AdminSessionHost, requestAdminSessionCheck } from "../../../api/session";
import { preserveProperty } from "../../../test-scope-fixture";
import type { PendingCommandHost } from "../../pending";
import { createCommandReload, usePendingCommands } from "../../pending/sources";
import { ADMIN_PAGES } from "../../router";
import { AdminShell } from "../index";
import { useAdminSessionHost } from "../shell";

/** 测试页的地址沿用 Rust 的命令夹具路径。 */
export const COMMAND_ROUTE = {
  name: "TestAuthenticatedCommands",
  pattern: "/test-authenticated-commands",
};

/** 观测命令派发、宿主交接、取消与回执交付。 */
export interface CommandRequests {
  readonly sent: string[];
  readonly hosts: {
    readonly session: AdminSessionHost;
    readonly pending: PendingCommandHost;
  }[];
  readonly replies: AdminApiFailure[];
  readonly failures: string[];
  cancelled: number;
  successes: number;
  sourceUnmounted: boolean;
}

/** 为每次挂载建立独立的命令观测记录。 */
export function commandRequests(): CommandRequests {
  return {
    sent: [],
    hosts: [],
    replies: [],
    failures: [],
    cancelled: 0,
    successes: 0,
    sourceUnmounted: false,
  };
}

const CommandRequestsContext = createContext<CommandRequests>();

function CommandPage(): JSX.Element {
  const router = useRouter();
  const observed = useContext(CommandRequestsContext);
  const host = useAdminSessionHost();
  const commands = usePendingCommands(createCommandReload(0n), () => true);
  observed.hosts.push({ session: host, pending: commands.host });
  onCleanup(() => {
    observed.sourceUnmounted = true;
  });

  function submit(
    entity: string,
    action: string,
    operation: (signal: AbortSignal) => Promise<Result<void, string>>,
  ): void {
    const started = commands.submit(
      entity,
      action,
      (id, signal) => {
        observed.sent.push(id);
        return operation(signal);
      },
      () => {
        // 命令宿主会捕获反馈异常，因此由用例在外层检查成功回调次数。
        observed.successes += 1;
      },
      (code) => observed.failures.push(code),
    );
    if (!started) {
      throw new Error(`命令未提交 ${entity}`);
    }
  }

  async function unavailable(): Promise<Result<void, string>> {
    const reply = adminApiFailureFromHttpBody(
      503,
      '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
    );
    observed.replies.push(reply);
    return err(reply.code);
  }

  function pending(signal: AbortSignal): Promise<Result<void, string>> {
    return new Promise((_resolve, reject) => {
      signal.addEventListener(
        "abort",
        () => {
          observed.cancelled += 1;
          reject(new DOMException("命令已取消", "AbortError"));
        },
        { once: true },
      );
    });
  }

  return (
    <>
      <h1>命令页</h1>
      <button type="button" onClick={() => router.push("/status")}>
        离开命令页
      </button>
      <button type="button" onClick={() => requestAdminSessionCheck(host)}>
        复核测试会话
      </button>
      <button type="button" onClick={() => submit("alert:fixture", "测试静默", unavailable)}>
        发送会话命令
      </button>
      <button type="button" onClick={() => submit("world:fixture", "测试世界命令", pending)}>
        保持命令在途
      </button>
    </>
  );
}

/** 仅登记测试命令页，Status 等生产页面继续使用原有路由表。 */
export function installCommandPage(): () => void {
  const restore = preserveProperty(ADMIN_PAGES, COMMAND_ROUTE.name);
  Object.defineProperty(ADMIN_PAGES, COMMAND_ROUTE.name, {
    configurable: true,
    enumerable: true,
    value: pageComponent(CommandPage),
  });
  return restore;
}

/** 用真实认证布局运行会话，返回按钮驱动同一个路由实例的历史后退。 */
export function CommandHostHarness(props: {
  readonly requests: CommandRequests;
  readonly router: Router;
  readonly locale: LocaleSession;
}): JSX.Element {
  const theme = useThemeEffects();
  return (
    <ThemeStateContext value={theme}>
      <LocaleSessionContext value={props.locale}>
        <CommandRequestsContext value={props.requests}>
          <RouterContext value={props.router}>
            <button type="button" onClick={() => props.router.goBack()}>
              返回命令页
            </button>
            <AdminShell />
          </RouterContext>
        </CommandRequestsContext>
      </LocaleSessionContext>
    </ThemeStateContext>
  );
}
