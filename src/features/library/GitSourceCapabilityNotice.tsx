import { useState } from "react";
import type {
  GitSourceCapabilityMember,
  GitSourceCapabilityReport,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";

export interface GitSourceCapabilityFailure {
  diagnostic: string | null;
}

export type SourceActionKind = "restore" | "copy" | "remove";

export interface SourceActionNotice {
  kind: SourceActionKind;
  ok: boolean;
  message: string | null;
}

/**
 * Native folder picker for the Local Source Copy destination. Dynamic
 * import is deliberate: @tauri-apps/plugin-dialog is platform-specific
 * (it only resolves inside the Tauri webview; component tests run in
 * plain jsdom without it), matching HomeBindingView's loader.
 */
export async function defaultCopyDestinationPicker(): Promise<string | null> {
  if (!("__TAURI_INTERNALS__" in window)) {
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selection = await open({ directory: true, multiple: false });
  return typeof selection === "string" ? selection : null;
}

export function GitSourceCapabilityNotice({
  report,
  failure,
  onPromote,
  onUpdate,
  actionActivity = false,
  actionNotice = null,
  onRestore,
  onCopyMember,
  onRemove,
  pickDirectory = defaultCopyDestinationPicker,
}: {
  report: GitSourceCapabilityReport | null;
  failure: GitSourceCapabilityFailure | null;
  onPromote?: (remoteId: string, trigger: HTMLButtonElement) => void;
  onUpdate?: (remoteId: string, trigger: HTMLButtonElement) => void;
  actionActivity?: boolean;
  actionNotice?: SourceActionNotice | null;
  onRestore?: (remoteId: string) => void;
  onCopyMember?: (
    remoteId: string,
    skillId: string,
    destination: string,
  ) => void;
  onRemove?: (remoteId: string) => void;
  pickDirectory?: () => Promise<string | null>;
}) {
  const { t } = useLocale();
  if (failure) {
    return (
      <section
        className="git-source-capability git-source-capability--unavailable"
        aria-label={t("library.source_capability.label")}
        role="alert"
      >
        <h2>{t("library.source_capability.unavailable")}</h2>
        <p>{t("library.source_capability.unavailable_body")}</p>
        {failure.diagnostic ? (
          <details>
            <summary>{t("library.source_capability.diagnostic")}</summary>
            <pre>{failure.diagnostic}</pre>
          </details>
        ) : null}
      </section>
    );
  }
  if (!report || report.sources.length === 0) return null;

  return (
    <section
      className="git-source-capability"
      aria-label={t("library.source_capability.label")}
    >
      <h2>{t("library.source_capability.heading")}</h2>
      <ul>
        {report.sources.map((source) => {
          return (
            <li
              key={source.remoteId}
              className={`git-source-capability--${source.kind}`}
            >
              <h3>{t(`library.source_capability.${source.kind}`)}</h3>
              <p className="git-source-capability-url">{source.canonicalUrl}</p>
              <p>{t(`library.source_capability.${source.kind}_detail`)}</p>
              {source.kind === "legacy_per_skill_git_state" && onPromote ? (
                <button
                  type="button"
                  className="repair-button"
                  onClick={(event) =>
                    onPromote(source.remoteId, event.currentTarget)
                  }
                >
                  {t("library.source_capability.promote")}
                </button>
              ) : null}
              {source.kind === "git_repository_source" ? (
                <GitRepositorySourceActions
                  remoteId={source.remoteId}
                  members={source.members}
                  actionActivity={actionActivity}
                  actionNotice={actionNotice}
                  onUpdate={onUpdate}
                  onRestore={onRestore}
                  onCopyMember={onCopyMember}
                  onRemove={onRemove}
                  pickDirectory={pickDirectory}
                />
              ) : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * Minimal #93 lifecycle surface for one Git Repository Source: Update,
 * Restore Current Source Release, Create Local Source Copy (per member,
 * user-chosen destination) and whole-source Remove. #90 owns the final
 * source group card and the bilingual close-out of these surfaces.
 */
function GitRepositorySourceActions({
  remoteId,
  members,
  actionActivity,
  actionNotice,
  onUpdate,
  onRestore,
  onCopyMember,
  onRemove,
  pickDirectory,
}: {
  remoteId: string;
  members: GitSourceCapabilityMember[];
  actionActivity: boolean;
  actionNotice: SourceActionNotice | null;
  onUpdate?: (remoteId: string, trigger: HTMLButtonElement) => void;
  onRestore?: (remoteId: string) => void;
  onCopyMember?: (
    remoteId: string,
    skillId: string,
    destination: string,
  ) => void;
  onRemove?: (remoteId: string) => void;
  pickDirectory: () => Promise<string | null>;
}) {
  const { t } = useLocale();
  const [confirming, setConfirming] = useState<"restore" | "remove" | null>(
    null,
  );
  const [copySkillId, setCopySkillId] = useState<string>(
    members.find((member) => member.presence)?.skillId ?? "",
  );

  const copyableMembers = members.filter((member) => member.presence);

  async function startCopy() {
    if (!onCopyMember || !copySkillId) return;
    const destination = await pickDirectory();
    if (destination) onCopyMember(remoteId, copySkillId, destination);
  }

  return (
    <div className="git-source-capability-actions">
      {onUpdate ? (
        <button
          type="button"
          className="repair-button"
          disabled={actionActivity}
          onClick={(event) => {
            setConfirming(null);
            onUpdate(remoteId, event.currentTarget);
          }}
        >
          {t("library.source_capability.update")}
        </button>
      ) : null}
      {onRestore ? (
        confirming === "restore" ? (
          <>
            <button
              type="button"
              className="repair-button"
              disabled={actionActivity}
              onClick={() => {
                setConfirming(null);
                onRestore(remoteId);
              }}
            >
              {t("library.source_capability.restore_confirm")}
            </button>
            <button
              type="button"
              disabled={actionActivity}
              onClick={() => setConfirming(null)}
            >
              {t("library.source_capability.cancel")}
            </button>
          </>
        ) : (
          <button
            type="button"
            className="repair-button"
            disabled={actionActivity}
            onClick={() => setConfirming("restore")}
          >
            {t("library.source_capability.restore")}
          </button>
        )
      ) : null}
      {onCopyMember && copyableMembers.length > 0 ? (
        <>
          {copyableMembers.length > 1 ? (
            <label className="git-source-capability-copy-member">
              <span>{t("library.source_capability.copy_member_label")}</span>
              <select
                value={copySkillId}
                disabled={actionActivity}
                onChange={(event) => setCopySkillId(event.target.value)}
              >
                {copyableMembers.map((member) => (
                  <option key={member.skillId} value={member.skillId}>
                    {member.skillPath}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          <button
            type="button"
            className="repair-button"
            disabled={actionActivity || copyableMembers.length === 0}
            onClick={() => void startCopy()}
          >
            {t("library.source_capability.copy_out")}
          </button>
        </>
      ) : null}
      {onRemove ? (
        confirming === "remove" ? (
          <>
            <button
              type="button"
              className="repair-button git-source-capability-remove-confirm"
              disabled={actionActivity}
              onClick={() => {
                setConfirming(null);
                onRemove(remoteId);
              }}
            >
              {t("library.source_capability.remove_confirm")}
            </button>
            <button
              type="button"
              disabled={actionActivity}
              onClick={() => setConfirming(null)}
            >
              {t("library.source_capability.cancel")}
            </button>
          </>
        ) : (
          <button
            type="button"
            className="repair-button"
            disabled={actionActivity}
            onClick={() => setConfirming("remove")}
          >
            {t("library.source_capability.remove")}
          </button>
        )
      ) : null}
      {actionNotice && actionNotice.message !== null ? (
        <p
          className="git-source-capability-action-result"
          role={actionNotice.ok ? "status" : "alert"}
        >
          {actionNotice.message}
        </p>
      ) : null}
    </div>
  );
}
