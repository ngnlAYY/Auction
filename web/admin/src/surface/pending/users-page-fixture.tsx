// 用户页整页测试的外壳：同时提供认证会话宿主与未决命令宿主。

import type { JSX } from "@solidjs/web";
import { createComponent } from "solid-js";

import { AdminSessionHostContext } from "../auth/shell";
import { type PendingCommandHost, PendingCommandHostContext } from ".";

/** 在真实认证外壳上下文里挂载用户页组件。 */
export function UsersPageFixture(props: {
  readonly host: PendingCommandHost;
  readonly page: () => JSX.Element;
}): JSX.Element {
  return (
    <AdminSessionHostContext value={props.host.sessionHost}>
      <PendingCommandHostContext value={props.host}>
        {createComponent(props.page, {})}
      </PendingCommandHostContext>
    </AdminSessionHostContext>
  );
}
