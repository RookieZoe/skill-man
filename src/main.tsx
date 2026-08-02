import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./app/App";
import { createCatalogClient } from "./app/catalog-client";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App client={createCatalogClient()} />
  </StrictMode>,
);
