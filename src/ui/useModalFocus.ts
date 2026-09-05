import { useEffect, useLayoutEffect, useRef } from "react";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button",
  "input",
  "select",
  "textarea",
  "[contenteditable='true']",
  "[tabindex]",
].join(",");

function focusableElements(root: HTMLElement): HTMLElement[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter((element) => {
    if (element.getAttribute("tabindex") === "-1") return false;
    if (
      element instanceof HTMLButtonElement ||
      element instanceof HTMLInputElement ||
      element instanceof HTMLSelectElement ||
      element instanceof HTMLTextAreaElement
    ) {
      return !element.disabled;
    }
    return !element.hidden && element.getAttribute("aria-hidden") !== "true";
  });
}

export function useModalFocus<T extends HTMLElement>({
  opener,
  busy = false,
  restoreFocus = true,
  focusKey,
  onClose,
}: {
  opener?: HTMLElement | null;
  busy?: boolean;
  restoreFocus?: boolean;
  focusKey?: unknown;
  onClose: () => void;
}) {
  const rootRef = useRef<T | null>(null);
  const openerRef = useRef<HTMLElement | null>(
    opener ??
      (typeof document !== "undefined" &&
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null),
  );

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const [first] = focusableElements(root);
    (first ?? root).focus();
  }, [focusKey]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      const root = rootRef.current;
      if (!root) return;

      if (event.key === "Escape") {
        if (busy) return;
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;

      const focusable = focusableElements(root);
      if (focusable.length === 0) {
        event.preventDefault();
        root.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!root.contains(document.activeElement)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [busy, onClose]);

  useEffect(
    () => () => {
      if (restoreFocus) openerRef.current?.focus();
    },
    [restoreFocus],
  );

  return rootRef;
}
