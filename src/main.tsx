import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { BootstrapApp } from "./app/BootstrapApp";
import { createCatalogClient } from "./app/catalog-client";
import "./styles.css";

const params = new URLSearchParams(window.location.search);
const showLayoutMatrix =
  import.meta.env.DEV && params.get("matrix") === "layout";
const showLayoutMatrixFrame =
  import.meta.env.DEV && params.get("matrix") === "layout-frame";

const root = createRoot(document.getElementById("root")!);

async function renderApplication() {
  if (import.meta.env.DEV && params.get("fixture") === "migration") {
    const { LocalMigrationDialog } =
      await import("./features/scan/LocalMigrationDialog");
    const { createFixtureCatalogClient } =
      await import("./test-fixtures/catalog");
    const client = createFixtureCatalogClient();
    client.planAdopt = async () => ({
      planToken: "preview",
      reportGeneration: 1,
      canApply: true,
      items: [
        {
          entityRef: "preview",
          action: "local_link_with_move",
          directoryName: "example-skill",
          canonicalEntity: "/Users/example/.agents/skills/example-skill",
          finalEntityPath: "/Users/example/Codes/local-skills/example-skill",
          appearances: [],
          activations: [
            {
              entryPath: "/Users/example/.agents/skills/example-skill",
              targetPath: "/Users/example/Codes/local-skills/example-skill",
            },
          ],
          applyable: true,
          error: null,
        },
      ],
    });
    root.render(
      <LocalMigrationDialog
        client={client}
        entityRef="preview"
        generation={1}
        name="example-skill"
        pickDirectory={async () => "/Users/example/Codes/local-skills"}
        formatError={String}
        onResolved={() => {}}
        onClose={() => root.render(null)}
      />,
    );
    return;
  }
  if (
    import.meta.env.DEV &&
    params.has("fixture") &&
    params.get("theme") === "dark"
  ) {
    (await import("./dev/preview-theme")).previewDarkTheme();
  }
  if (showLayoutMatrix) {
    const { LayoutMatrix } = await import("./dev/layout-matrix/LayoutMatrix");
    root.render(
      <StrictMode>
        <LayoutMatrix />
      </StrictMode>,
    );
    return;
  }
  if (showLayoutMatrixFrame) {
    const { LayoutMatrixFrame } =
      await import("./dev/layout-matrix/LayoutMatrixFrame");
    root.render(
      <StrictMode>
        <LayoutMatrixFrame />
      </StrictMode>,
    );
    return;
  }

  root.render(
    <StrictMode>
      <BootstrapApp
        client={
          import.meta.env.DEV && params.get("fixture") === "scan"
            ? (
                await import("./test-fixtures/scan-report")
              ).createScanPreviewClient()
            : import.meta.env.DEV && params.get("fixture") === "git-preview"
              ? (
                  await import("./test-fixtures/git-preview")
                ).createGitPreviewClient(params.get("removed") === "true")
              : import.meta.env.DEV && params.get("fixture") === "enable"
                ? (
                    await import("./test-fixtures/catalog")
                  ).createFixtureCatalogClient()
                : createCatalogClient()
        }
      />
    </StrictMode>,
  );
}

void renderApplication();
