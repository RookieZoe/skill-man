import { act, render } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { DetectionCardList } from "./DetectionCardList";
import "../../styles.css";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

test("fills only the last flex row and recomputes slots after resize and removal", () => {
  let width = 1250;
  let measure = () => {};
  const disconnect = vi.fn();
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(
    () => width,
  );
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        measure = callback;
      }
      observe() {}
      disconnect = disconnect;
    },
  );
  const cards = (count: number) =>
    Array.from({ length: count }, (_, index) => (
      <article key={index} className="agent-detection-card">
        Agent {index}
      </article>
    ));
  const view = render(<DetectionCardList>{cards(5)}</DetectionCardList>);
  const slots = () => view.container.querySelectorAll(".agent-detection-slot");
  expect(slots()).toHaveLength(1);
  expect(slots()[0]).toHaveAttribute("aria-hidden", "true");
  expect(
    getComputedStyle(view.container.firstElementChild!).justifyContent,
  ).toBe("space-between");
  expect(
    getComputedStyle(view.container.querySelector("article")!).maxWidth,
  ).toBe("400px");
  view.rerender(<DetectionCardList>{cards(1)}</DetectionCardList>);
  expect(slots()).toHaveLength(2);
  act(() => {
    width = 850;
    measure();
  });
  expect(slots()).toHaveLength(1);
  view.rerender(<DetectionCardList>{cards(4)}</DetectionCardList>);
  expect(slots()).toHaveLength(0);
  act(() => {
    width = 375;
    measure();
  });
  expect(slots()).toHaveLength(0);
  view.rerender(<DetectionCardList>{cards(0)}</DetectionCardList>);
  expect(slots()).toHaveLength(0);
  view.unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});
