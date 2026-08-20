import { useEffect, useRef, useState } from "react";

import "./layout-matrix.css";

type PresetKey =
  "wide" | "above" | "below" | "minimum" | "subminimum" | "short" | "custom";

const PRESETS: Record<
  Exclude<PresetKey, "custom">,
  { width: number; height: number; label: string }
> = {
  wide: { width: 1180, height: 760, label: "1180×760 · wide" },
  above: { width: 1060, height: 600, label: "1060×600 · above breakpoint" },
  below: { width: 1059, height: 600, label: "1059×600 · below breakpoint" },
  minimum: { width: 760, height: 520, label: "760×520 · native minimum" },
  subminimum: { width: 759, height: 520, label: "759×520 · defensive" },
  short: { width: 900, height: 420, label: "900×420 · short window" },
};

const REGION_SELECTORS = [
  ["Toolbar", ".toolbar"],
  ["NoticeRegion", ".notice-region"],
  ["Workspace", ".library-desk"],
  ["Library", ".library-sidebar"],
  ["Skill detail", ".skill-detail"],
  ["Agent inspector", ".agent-inspector"],
  ["Agent backdrop", ".agent-drawer-backdrop"],
  ["Sheet backdrop", ".activation-sheet-backdrop"],
  ["Operation status", ".operation-status-window"],
] as const;

interface RegionMetric {
  name: string;
  height: number;
  scrollHeight: number;
  scrollTop: number;
  overflowY: string;
}

interface MatrixMetrics {
  mode: string;
  notices: number;
  drawer: string;
  focus: string;
  regions: RegionMetric[];
  workspaceBottomGap: number;
  horizontalOverflow: number;
}

const EMPTY_METRICS: MatrixMetrics = {
  mode: "—",
  notices: 0,
  drawer: "closed",
  focus: "—",
  regions: [],
  workspaceBottomGap: 0,
  horizontalOverflow: 0,
};

function accessibleName(element: Element | null) {
  if (!(element instanceof HTMLElement)) return "none";
  return (
    element.getAttribute("aria-label") ??
    element.textContent?.trim().replace(/\s+/g, " ").slice(0, 72) ??
    element.tagName.toLowerCase()
  );
}

export function LayoutMatrix() {
  const [preset, setPreset] = useState<PresetKey>("wide");
  const [width, setWidth] = useState(PRESETS.wide.width);
  const [height, setHeight] = useState(PRESETS.wide.height);
  const [content, setContent] = useState("normal");
  const [density, setDensity] = useState("dense");
  const [language, setLanguage] = useState("en");
  const [notices, setNotices] = useState("none");
  const [onboarding, setOnboarding] = useState(false);
  const [operation, setOperation] = useState(false);
  const [metrics, setMetrics] = useState<MatrixMetrics>(EMPTY_METRICS);
  const frameRef = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    document.body.classList.add("lm-lab-active");
    return () => document.body.classList.remove("lm-lab-active");
  }, []);

  function applyPreset(next: PresetKey) {
    setPreset(next);
    if (next !== "custom") {
      const frame = PRESETS[next];
      setWidth(frame.width);
      setHeight(frame.height);
    }
  }

  const frameParams = new URLSearchParams({
    matrix: "layout-frame",
    content,
    density,
    language,
    notices,
    operation: operation ? "adopt" : "none",
  });
  if (onboarding) frameParams.set("onboarding", "1");
  const frameUrl = `/?${frameParams}`;

  function measure() {
    const frame = frameRef.current;
    const doc = frame?.contentDocument;
    const window = frame?.contentWindow;
    if (!frame || !doc || !window) return;
    const regions = REGION_SELECTORS.flatMap(([name, selector]) => {
      const element = doc.querySelector<HTMLElement>(selector);
      if (!element) return [];
      return [
        {
          name,
          height: Math.round(element.getBoundingClientRect().height),
          scrollHeight: element.scrollHeight,
          scrollTop: Math.round(element.scrollTop),
          overflowY: window.getComputedStyle(element).overflowY,
        },
      ];
    });
    const desk = doc.querySelector<HTMLElement>(".library-desk");
    const deskRect = desk?.getBoundingClientRect();
    setMetrics({
      mode:
        doc.querySelector(".app-shell")?.getAttribute("data-layout-mode") ??
        "—",
      notices: doc.querySelector(".notice-region")?.children.length ?? 0,
      drawer:
        doc.querySelector(".agent-drawer")?.getAttribute("data-open") === "true"
          ? "open"
          : "closed",
      focus: accessibleName(doc.activeElement),
      regions,
      workspaceBottomGap: deskRect
        ? Math.max(
            0,
            doc.documentElement.clientHeight - Math.round(deskRect.bottom),
          )
        : 0,
      horizontalOverflow: Math.max(
        0,
        doc.documentElement.scrollWidth - doc.documentElement.clientWidth,
      ),
    });
  }

  useEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const observer = new ResizeObserver(measure);
    observer.observe(frame);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="lm-lab">
      <header className="lm-heading">
        <div>
          <span>LAYOUT MATRIX · DEV ONLY</span>
          <h1>Pinned Workbench — accepted layout contract</h1>
          <p>
            The frame runs the production Library Desk with an injected scenario
            client. Resizing the frame is a real window resize for the app; open
            a sheet or the Agent drawer, then switch presets to verify no
            remount, stable focus, and preserved scroll owners.
          </p>
        </div>
        <button type="button" className="lm-measure" onClick={measure}>
          Measure
        </button>
      </header>
      <div className="lm-layout">
        <aside className="lm-controls" aria-label="Matrix controls">
          <fieldset>
            <legend>Window size</legend>
            <div className="lm-presets">
              {(
                Object.keys(PRESETS) as Array<Exclude<PresetKey, "custom">>
              ).map((key) => (
                <button
                  type="button"
                  key={key}
                  aria-pressed={preset === key}
                  onClick={() => applyPreset(key)}
                >
                  {PRESETS[key].label}
                </button>
              ))}
            </div>
            <div className="lm-size-inputs">
              <label>
                Width
                <input
                  type="number"
                  value={width}
                  min={320}
                  onChange={(event) => {
                    applyPreset("custom");
                    setWidth(Number(event.currentTarget.value));
                  }}
                />
              </label>
              <label>
                Height
                <input
                  type="number"
                  value={height}
                  min={320}
                  onChange={(event) => {
                    applyPreset("custom");
                    setHeight(Number(event.currentTarget.value));
                  }}
                />
              </label>
            </div>
          </fieldset>
          <fieldset>
            <legend>Scenario</legend>
            <label>
              Content
              <select
                value={content}
                onChange={(event) => setContent(event.currentTarget.value)}
              >
                <option value="normal">Normal catalog</option>
                <option value="empty">Empty Library</option>
                <option value="error">Catalog error</option>
              </select>
            </label>
            <label>
              Density
              <select
                value={density}
                onChange={(event) => setDensity(event.currentTarget.value)}
              >
                <option value="standard">Standard copy</option>
                <option value="dense">Dense long copy</option>
              </select>
            </label>
            <label>
              Source language
              <select
                value={language}
                onChange={(event) => setLanguage(event.currentTarget.value)}
              >
                <option value="en">English source content</option>
                <option value="zh">简体中文 source content</option>
              </select>
            </label>
            <label>
              Notices
              <select
                value={notices}
                onChange={(event) => setNotices(event.currentTarget.value)}
              >
                <option value="none">None (region collapses)</option>
                <option value="lock">One (recovery lock)</option>
                <option value="stack">Two (error + lock tray)</option>
              </select>
            </label>
            <label className="lm-check">
              <input
                type="checkbox"
                checked={onboarding}
                onChange={(event) => setOnboarding(event.currentTarget.checked)}
              />
              Tall onboarding overlay (low-height scroll evidence)
            </label>
            <label className="lm-check">
              <input
                type="checkbox"
                checked={operation}
                onChange={(event) => setOperation(event.currentTarget.checked)}
              />
              Active operation window
            </label>
          </fieldset>
          <p className="lm-hint">
            The frame remounts only when a scenario select changes; window
            presets resize the iframe in place.
          </p>
        </aside>
        <div className="lm-stage">
          <iframe
            ref={frameRef}
            className="lm-frame"
            title="Library Desk matrix frame"
            style={{ width, height }}
            src={frameUrl}
            onLoad={measure}
          />
          <section className="lm-hud" aria-label="Layout metrics">
            <div className="lm-hud-facts">
              <span>
                Mode <strong>{metrics.mode}</strong>
              </span>
              <span>
                Notices <strong>{metrics.notices}</strong>
              </span>
              <span>
                Drawer <strong>{metrics.drawer}</strong>
              </span>
              <span>
                Focus <strong title={metrics.focus}>{metrics.focus}</strong>
              </span>
              <span>
                Workspace gap <strong>{metrics.workspaceBottomGap}px</strong>
              </span>
              <span>
                Page x-overflow <strong>{metrics.horizontalOverflow}px</strong>
              </span>
            </div>
            <table>
              <thead>
                <tr>
                  <th>Region</th>
                  <th>Height</th>
                  <th>scrollHeight</th>
                  <th>scrollTop</th>
                  <th>overflowY</th>
                </tr>
              </thead>
              <tbody>
                {metrics.regions.map((region) => (
                  <tr key={region.name}>
                    <td>{region.name}</td>
                    <td>{region.height}</td>
                    <td>{region.scrollHeight}</td>
                    <td>{region.scrollTop}</td>
                    <td>{region.overflowY}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        </div>
      </div>
    </div>
  );
}
