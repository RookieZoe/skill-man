import {
  Children,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";

/** Equal flex columns, including empty slots in the last row. */
export function DetectionCardList({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [layout, setLayout] = useState({ columns: 1, width: 400 });
  const cards = Children.toArray(children);

  useLayoutEffect(() => {
    const list = ref.current;
    if (!list) return;
    const measure = () => {
      const gap = Number.parseFloat(getComputedStyle(list).columnGap) || 0;
      const columns = Math.max(
        1,
        Math.floor((list.clientWidth + gap) / (320 + gap)),
      );
      const width = Math.min(
        400,
        Math.max(0, (list.clientWidth - gap * (columns - 1)) / columns),
      );
      setLayout((previous) =>
        previous.columns === columns && previous.width === width
          ? previous
          : { columns, width },
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measure);
      return () => window.removeEventListener("resize", measure);
    }
    const observer = new ResizeObserver(measure);
    observer.observe(list);
    return () => observer.disconnect();
  }, []);

  const emptySlots =
    (layout.columns - (cards.length % layout.columns)) % layout.columns;
  return (
    <div
      ref={ref}
      className="agent-detection-list"
      style={{ "--detection-card-width": `${layout.width}px` } as CSSProperties}
    >
      {cards}
      {Array.from({ length: emptySlots }, (_, index) => (
        <div
          key={`empty-${index}`}
          className="agent-detection-slot"
          aria-hidden="true"
        />
      ))}
    </div>
  );
}
