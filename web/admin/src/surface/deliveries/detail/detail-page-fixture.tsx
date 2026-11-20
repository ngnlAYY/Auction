// 同一个会话中把已存在的列表命令来源与投递详情一起挂载。

import { err } from "@auction/web-shared/result";
import { RouteOutlet } from "@auction/web-shared/router";
import type { JSX } from "@solidjs/web";
import { writeFailureFromReply } from "../../../api/api-request-fixture";
import { createCommandReload, usePendingCommands } from "../../pending/sources";
import { ADMIN_PAGES } from "../../router";
import { deliveryCommandEntity } from "..";
import { detailPage } from ".";

/** 列表来源通过真实按钮占住详情页同一个投递实体。 */
export function DetailWithListCommand(props: {
  readonly id: string;
  readonly onSubmitResult: (submitted: boolean) => void;
}): JSX.Element {
  const list = usePendingCommands(createCommandReload(), () => false);
  return (
    <>
      <button
        type="button"
        onClick={() => {
          const failure = () =>
            writeFailureFromReply(
              503,
              '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
            );
          const submitted = list.submit(
            deliveryCommandEntity(props.id),
            "列表处置",
            async () => err(await failure()),
            () => {},
            () => {},
          );
          props.onSubmitResult(submitted);
        }}
      >
        列表提交同一投递
      </button>
      {detailPage(props.id)}
    </>
  );
}

/** 详情书签经实际 Admin 路由表加载，认证与 DOM 沿用投递夹具。 */
export function DeliveryRouteFixture(): JSX.Element {
  return <RouteOutlet pages={ADMIN_PAGES} notFound={null} />;
}
