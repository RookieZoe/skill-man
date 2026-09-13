import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  BootstrapSnapshot,
  CatalogClient,
  SkillSummary,
} from "../../app/catalog-client";
import { LocaleProvider, useLocale } from "../locale/LocaleProvider";
import { AppearanceProvider } from "../appearance/AppearanceProvider";
import { TraySource } from "./TraySource";
import { TrayReading } from "./TrayReading";

export interface TrayPanelProps {
  client: CatalogClient;
  onClose: () => void;
  onOpenMain: () => void;
  onQuit: () => void;
  onReady?: () => void;
  nativeError?: boolean;
  copyMarkdown?: (markdown: string) => Promise<void>;
}

export function TrayPanel(props: TrayPanelProps) {
  return (
    <LocaleProvider client={props.client}>
      <AppearanceProvider>
        <Panel {...props} />
      </AppearanceProvider>
    </LocaleProvider>
  );
}

const normalized = (text: string) => text.normalize("NFKC").toLowerCase();
const copyToClipboard = (markdown: string) =>
  navigator.clipboard.writeText(markdown);

function Panel({
  client,
  onClose,
  onOpenMain,
  onQuit,
  onReady,
  nativeError,
  copyMarkdown = copyToClipboard,
}: TrayPanelProps) {
  const { t } = useLocale();
  const [items, setItems] = useState<SkillSummary[]>([]);
  const [availability, setAvailability] = useState("loading");
  const [revision, setRevision] = useState(0);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [reading, setReading] = useState<string | null>(null);
  const search = useRef<HTMLInputElement>(null);
  const listElement = useRef<HTMLDivElement>(null);
  const scroll = useRef(0);
  useEffect(() => {
    let current = true;
    let generation = -1;
    let epoch = 0;
    let home: string | null = null;
    const stops: (() => void)[] = [];
    const accept = async (snapshot: BootstrapSnapshot) => {
      const version = ++epoch;
      setAvailability("loading");
      const nextHome = snapshot.state === "bound" ? snapshot.homeId : null;
      if (home !== nextHome) {
        home = nextHome;
        setItems([]);
        setReading(null);
        setQuery("");
        setSelected(null);
        scroll.current = 0;
      }
      if (
        snapshot.state !== "bound" ||
        snapshot.catalogReadonlyReason === "integrity_failed"
      ) {
        setItems([]);
        setReading(null);
        setAvailability(
          snapshot.state === "bound" ? "unavailable" : snapshot.state,
        );
        return;
      }
      try {
        const list = await client.listSkills("all");
        if (current && version === epoch) {
          setItems(list.items);
          setAvailability("ready");
          setRevision((value) => value + 1);
        }
      } catch {
        if (current && version === epoch) {
          setItems([]);
          setReading(null);
          setAvailability("unavailable");
        }
      }
    };
    const refresh = async () => {
      const version = ++epoch;
      setAvailability("loading");
      try {
        const snapshot = await client.getBootstrapSnapshot();
        if (current && version === epoch) await accept(snapshot);
      } catch {
        if (current && version === epoch) {
          setItems([]);
          setReading(null);
          setAvailability("unavailable");
        }
      }
    };
    const subscribe = async () => {
      for (const start of [
        () =>
          client.listenBootstrapChanged((payload) => {
            if (current && payload.generation >= generation) {
              generation = payload.generation;
              void accept(payload.snapshot);
            }
          }),
        () =>
          client.listenCatalogChanged(() => {
            if (current) void refresh();
          }),
      ]) {
        try {
          const stop = await start();
          if (current) stops.push(stop);
          else stop();
        } catch {
          if (current) setAvailability("unavailable");
          return;
        }
      }
      if (current) await refresh();
    };
    void subscribe();
    return () => {
      current = false;
      epoch++;
      stops.forEach((stop) => stop());
    };
  }, [client]);
  useEffect(() => {
    onReady?.();
  }, [onReady]);
  const needle = normalized(query.trim());
  const results = (availability === "ready" ? items : [])
    .filter((item) =>
      [item.displayName, item.directoryName, item.description].some((value) =>
        normalized(value).includes(needle),
      ),
    )
    .sort(
      (a, b) =>
        a.displayName.localeCompare(b.displayName, "en") ||
        a.id.localeCompare(b.id, "en"),
    );
  const active = results.find((item) => item.id === selected) ?? results[0];
  useLayoutEffect(() => {
    if (!reading) {
      search.current?.focus();
      if (listElement.current) listElement.current.scrollTop = scroll.current;
    }
  }, [reading, revision]);
  const back = () => {
    setReading(null);
    search.current?.focus();
  };
  const open = (id: string) => {
    scroll.current = listElement.current?.scrollTop ?? 0;
    setSelected(id);
    setReading(id);
  };
  return (
    <main
      className="tray-panel"
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing || event.keyCode === 229) return;
        if (event.key === "Escape") {
          event.preventDefault();
          if (reading) back();
          else onClose();
        }
        if (event.metaKey && event.key.toLowerCase() === "k") {
          event.preventDefault();
          back();
        }
        if (
          !reading &&
          (event.target === search.current ||
            event.target === listElement.current ||
            (event.target as HTMLElement).getAttribute("role") === "option")
        ) {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const index = selected
              ? results.findIndex((item) => item.id === active?.id)
              : -1;
            const next =
              results[
                Math.max(
                  0,
                  Math.min(
                    results.length - 1,
                    index + (event.key === "ArrowDown" ? 1 : -1),
                  ),
                )
              ];
            if (next) {
              setSelected(next.id);
              document
                .getElementById(`tray-skill-${next.id}`)
                ?.scrollIntoView?.({ block: "nearest" });
            }
          }
          if (event.key === "Enter" && active) {
            event.preventDefault();
            void open(active.id);
          }
        }
      }}
    >
      {reading && availability === "ready" ? (
        <TrayReading
          key={`${reading}:${revision}`}
          client={client}
          skillId={reading}
          summary={items.find((item) => item.id === reading)}
          onBack={back}
          copyMarkdown={copyMarkdown}
        />
      ) : (
        <>
          <input
            ref={search}
            type="search"
            aria-label={t("tray.panel.search")}
            placeholder={t("tray.panel.search")}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setSelected(null);
              scroll.current = 0;
              if (listElement.current) listElement.current.scrollTop = 0;
            }}
          />
          <div
            ref={listElement}
            onScroll={(event) => {
              if (availability === "ready")
                scroll.current = event.currentTarget.scrollTop;
            }}
            role="listbox"
            tabIndex={0}
            aria-label={t("tray.panel.skills")}
            aria-activedescendant={
              active ? `tray-skill-${active.id}` : undefined
            }
          >
            {results.map((item) => (
              <div
                id={`tray-skill-${item.id}`}
                role="option"
                tabIndex={-1}
                aria-selected={item.id === active?.id}
                key={item.id}
                onClick={() => void open(item.id)}
              >
                <strong>{item.displayName}</strong>
                <TraySource
                  key={`${item.id}:${revision}`}
                  client={client}
                  skill={item}
                  ambiguous={
                    items.filter(
                      (other) => other.displayName === item.displayName,
                    ).length > 1
                  }
                />
                <p>{item.description}</p>
              </div>
            ))}
          </div>
          {availability !== "ready" ? (
            <p role="status">
              {t(
                availability === "loading"
                  ? "tray.panel.loading"
                  : availability === "home_unavailable"
                    ? "tray.panel.home_unavailable"
                    : availability === "home_identity_mismatch"
                      ? "tray.panel.identity_mismatch"
                      : "tray.panel.unavailable",
              )}
            </p>
          ) : (
            results.length === 0 && (
              <p role="status">
                {t(
                  items.length === 0
                    ? "tray.panel.empty"
                    : "tray.panel.no_results",
                )}
              </p>
            )
          )}
          {query && (
            <button
              onClick={() => {
                setQuery("");
                search.current?.focus();
              }}
            >
              {t("tray.panel.clear")}
            </button>
          )}
        </>
      )}
      {nativeError && <p role="alert">{t("tray.panel.window_failed")}</p>}
      <footer>
        <button onClick={onOpenMain}>{t("tray.open_window")}</button>
        <button onClick={onClose}>{t("tray.panel.close")}</button>
        <button onClick={onQuit}>{t("tray.quit")}</button>
      </footer>
    </main>
  );
}
