import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// jsdom defaults to 1024×768, which would render the Library Desk in mid
// mode with a closed Agent drawer. Default to a wide viewport so existing
// tests exercise the three-pane contract; layout tests override explicitly.
Object.defineProperty(window, "innerWidth", {
  value: 1280,
  configurable: true,
});
Object.defineProperty(window, "innerHeight", {
  value: 800,
  configurable: true,
});

afterEach(cleanup);

Object.defineProperty(window, "matchMedia", {
  configurable: true,
  writable: true,
  value: (media: string) => ({
    matches: false,
    media,
    addEventListener() {},
    removeEventListener() {},
  }),
});
