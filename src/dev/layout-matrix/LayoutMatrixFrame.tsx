import { App } from "../../app/App";
import { OperationStatusWindow } from "../../ui/OperationStatusWindow";
import {
  createMatrixCatalogClient,
  scenarioFromParams,
  scenarioKey,
} from "./scenario-client";

/**
 * Matrix frame page: the real production Library Desk with an explicitly
 * injected scenario client. Rendered inside the lab iframe so the frame's
 * window size is the app's viewport — resizing the iframe is a genuine
 * window-resize event for the app (spec §4.7 explicit injection only).
 */
export function LayoutMatrixFrame() {
  const params = new URLSearchParams(window.location.search);
  const scenario = scenarioFromParams(params);
  const client = createMatrixCatalogClient(scenario);
  const operationPreview =
    params.get("language") === "zh"
      ? {
          ariaLabel: "当前操作",
          heading: "正在进行 1 项操作",
          operation: {
            id: "adopt-scan",
            title: "正在扫描",
            detail:
              "正在读取已配置的智能体和共享技能目录。只有 lock 要求时才会验证远程来源。",
          },
        }
      : {
          ariaLabel: "Current activity",
          heading: "1 operation in progress",
          operation: {
            id: "adopt-scan",
            title: "Scanning",
            detail:
              "Reading configured Agent and shared Skill directories. Remote sources are verified only when a lock requires it.",
          },
        };
  return (
    <>
      <App key={scenarioKey(scenario)} client={client} />
      {params.get("operation") === "adopt" ? (
        <OperationStatusWindow
          ariaLabel={operationPreview.ariaLabel}
          heading={operationPreview.heading}
          operations={[operationPreview.operation]}
        />
      ) : null}
    </>
  );
}
