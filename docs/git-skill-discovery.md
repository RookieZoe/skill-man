# Git Skill discovery compatibility

Git repository discovery follows the default discovery behavior of `skills` CLI
1.5.23 (upstream commit `435076e78988e1e6ec40d00b0b1d76bdbbc5419a`). This is
discovery compatibility, not a change to Source Tracking Policy: release, stable
SemVer tag, reachable tag, and HEAD resolution retain their existing precedence.
Different selected commits can therefore legitimately contain different members.

- A valid root Skill wins; discovery stops below a Skill document.
- Common Skill directories and local plugin manifests guide bounded discovery,
  with bounded recursive fallback when those searches find no Skills.
- Valid string name and description frontmatter are required. Internal Skills
  and project-installed dependencies are excluded by default. Duplicate display
  names retain the first discovered member.
- Plugin categories come from local plugin manifest Skill mappings. Unmatched
  members appear under Other only when the repository has categorized members.
  Repositories without category metadata retain an ungrouped member list.

Categories are display metadata, not directory identity or ownership evidence.
Confirmed imports and updates freeze path-to-plugin mappings in the parent
manifest alongside the selected release. Preview, repository management, and
library views use those mappings. Existing manifests without mappings remain
readable and ungrouped; no implicit migration or external-directory rewrite is
performed. Undo and recovery retain their frozen-manifest semantics.

During external ownership replacement, the canonical repository and normalized
lock `skillPath` identify a member, independently of the installer entry name.
A unique validated claim preserves that entry name as the managed Directory
Identity (for example `mmx-cli` for `skill/SKILL.md`). Subsequent updates retain
the stored identity rather than deriving it again from the repository basename.
Multiple claims for the same member path are rejected before writes. Recovery
and Undo validate the frozen installer name and original external path.

If an external claim's old repository path is absent, replacement can match a
single current member whose directory and display name equal the installer
entry, provided the readable local Skill metadata confirms that name. Exact
path matches take precedence, and claims must resolve one-to-one. The mapping
is frozen separately in the journal; original lock entries are never rewritten.
Replacement republishes the original local activation entry, while recovery
uses the frozen mapping without refetching and Undo restores the original lock.

If neither the old path nor a unique same-name member exists, the old Skill is
removed and any unmatched remote member is added; this is not a rename mapping
(for example `design` removed and `ui` added). Preview lists removed installer
names and new members. Ordinary replacement stays blocked until the user
acknowledges the exact removal list and chooses **Force remote replacement**.
Core recomputes that list against the selected immutable commit and current
lock; missing, duplicate or stale acknowledgments are rejected before writes.
Force does not bypass ownership, path, fingerprint, or lock-CAS checks.

The transition isolates removed external entities for rollback/Undo and removes
their verified direct enablement links. Existing matched members preserve their
activation state. New members are installed only into App Home and are never
automatically enabled; the completion view directs the user to the Library for
manual enablement. Recovery uses frozen journal facts without refetching, and
Undo restores the old entities, links and exact installer lock entries together.

Repository-controlled documents are read lazily within existing size limits;
unused YAML fields are skipped without materializing alias expansions. Plugin
paths cannot escape the repository. Existing conservative symlink checks remain
in force.
