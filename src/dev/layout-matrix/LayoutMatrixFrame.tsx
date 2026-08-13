import { App } from "../../app/App";
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
  const scenario = scenarioFromParams(
    new URLSearchParams(window.location.search),
  );
  const client = createMatrixCatalogClient(scenario);
  return <App key={scenarioKey(scenario)} client={client} />;
}
