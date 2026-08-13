import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { BootstrapApp } from "./app/BootstrapApp";
import { createCatalogClient } from "./app/catalog-client";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BootstrapApp client={createCatalogClient()} />
  </StrictMode>,
);
