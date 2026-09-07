import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useLocale } from "../features/locale/LocaleProvider";
import {
  OperationStatusWindow,
  type OperationStatus,
} from "./OperationStatusWindow";

type Notice = Omit<OperationStatus, "id">;
interface Operations {
  begin: (notice: Notice) => string;
  update: (id: string, notice: Notice) => void;
  finish: (id: string, notice: Notice) => void;
  dismiss: (id: string) => void;
}
const OperationsContext = createContext<Operations>({
  begin: () => "",
  update: () => {},
  finish: () => {},
  dismiss: () => {},
});

/** Explicit completion only: disappearing UI or rejected work is never success. */
export function BackgroundOperations({ children }: { children: ReactNode }) {
  const { t, tPlural } = useLocale();
  const nextId = useRef(0);
  const [operations, setOperations] = useState<OperationStatus[]>([]);
  const dismiss = useCallback((id: string) => {
    setOperations((items) => items.filter((item) => item.id !== id));
  }, []);
  const begin = useCallback((notice: Notice) => {
    const id = `operation-${++nextId.current}`;
    setOperations((items) => [...items, { ...notice, id, state: "running" }]);
    return id;
  }, []);
  const update = useCallback((id: string, notice: Notice) => {
    setOperations((items) =>
      items.map((item) => (item.id === id ? { ...item, ...notice } : item)),
    );
  }, []);
  const finish = useCallback((id: string, notice: Notice) => {
    setOperations((items) =>
      items.map((item) =>
        item.id === id
          ? { ...item, ...notice, state: notice.state ?? "completed" }
          : item,
      ),
    );
  }, []);
  const value = useMemo(
    () => ({ begin, update, finish, dismiss }),
    [begin, update, finish, dismiss],
  );
  const running = operations.filter((item) => item.state === "running").length;
  return (
    <OperationsContext.Provider value={value}>
      {children}
      {operations.length > 0 && (
        <OperationStatusWindow
          ariaLabel={t("operation.status.label")}
          heading={
            running
              ? tPlural("operation.status.running", running, { count: running })
              : t("operation.status.results")
          }
          operations={operations}
          onDismiss={dismiss}
          dismissLabel={t("operation.status.dismiss")}
        />
      )}
    </OperationsContext.Provider>
  );
}

export function useBackgroundOperations() {
  return useContext(OperationsContext);
}
