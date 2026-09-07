import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { useModalFocus } from "./useModalFocus";

function Sheet({ step }: { step: string }) {
  const ref = useModalFocus({ focusKey: step, onClose() {} });
  return (
    <section ref={ref} role="dialog">
      <div className="enable-sheet-body">
        <button>{step}</button>
      </div>
    </section>
  );
}

test("changing steps resets both dialog and nested body scrolling", () => {
  const { rerender } = render(<Sheet step="preview" />);
  const dialog = screen.getByRole("dialog");
  const body = dialog.querySelector<HTMLElement>(".enable-sheet-body")!;
  dialog.scrollTop = 900;
  body.scrollTop = 600;
  rerender(<Sheet step="result" />);
  expect(dialog.scrollTop).toBe(0);
  expect(body.scrollTop).toBe(0);
  expect(screen.getByRole("button", { name: "result" })).toHaveFocus();
});
