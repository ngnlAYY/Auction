// 认证副作用测试的真实表单装配：登录状态仍由 `useSession` 持有，表单使用公开组件。

import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { useSession } from "./effects";
import { AdminLogin } from "./login";
import { AdminSessionHostContext } from "./shell";

/** 登录表单外壳需要的会话句柄。 */
export type SessionHandle = ReturnType<typeof useSession>;

/** 已登出时挂载真实 `AdminLogin`；登录中或已认证时卸载表单。 */
export function LoginHarness(props: { readonly session: SessionHandle }): JSX.Element {
  const view = props.session.view;
  return (
    <AdminSessionHostContext value={props.session.host}>
      <button type="button" onClick={() => props.session.dispatch({ kind: "broadcast" })}>
        广播结束会话
      </button>
      <Show when={view().loggedOut} fallback={<p>已认证</p>}>
        <AdminLogin
          submitting={view().loginPending}
          loginError={view().loginError}
          onEdit={() => props.session.dispatch({ kind: "login_input_changed" })}
          onSubmit={(submission) => props.session.submitLogin(submission)}
        />
      </Show>
    </AdminSessionHostContext>
  );
}
