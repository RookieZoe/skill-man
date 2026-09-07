//! Git-tree discovery compatible with skills 1.5.23 (435076e7).
//! The caller owns version selection and transport. Paths and plugin labels
//! here are repository-relative facts, never filesystem authority.
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};

// Typed deserialization deliberately skips unused fields via IgnoredAny:
// unlike Value, it does not expand alias trees in arbitrary metadata.
#[derive(Deserialize)]
struct Frontmatter {
    name: Text,
    description: Text,
    #[serde(default)]
    metadata: Option<InternalMetadata>,
}
#[derive(Deserialize)]
struct InternalMetadata {
    #[serde(default)]
    internal: bool,
}
struct Text(String);
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TextVisitor;
        impl<'de> serde::de::Visitor<'de> for TextVisitor {
            type Value = Text;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Text, E> {
                Ok(Text(value.to_owned()))
            }
        }
        deserializer.deserialize_any(TextVisitor)
    }
}

const AGENT_DIRS: &[&str] = &[
    ".agents/skills",
    ".claude/skills",
    ".cline/skills",
    ".codebuddy/skills",
    ".codex/skills",
    ".commandcode/skills",
    ".continue/skills",
    ".github/skills",
    ".goose/skills",
    ".grok/skills",
    ".iflow/skills",
    ".junie/skills",
    ".kilocode/skills",
    ".kimchi/skills",
    ".kiro/skills",
    ".minimax/skills",
    ".mux/skills",
    ".neovate/skills",
    ".opencode/skills",
    ".openhands/skills",
    ".pi/skills",
    ".posit/assistant/skills",
    ".qoder/skills",
    ".roo/skills",
    ".trae/skills",
    ".windsurf/skills",
    ".zcode/skills",
    ".zencoder/skills",
];
const SKIP: &[&str] = &["node_modules", ".git", "dist", "build", "__pycache__"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositorySkill {
    pub path: String,
    pub name: String,
    pub description: String,
    pub plugin_name: Option<String>,
}

/// Parse complete YAML, including quoted and folded metadata. Unlike the
/// permissive Local Source display parser, Git discovery requires both fields.
pub(crate) fn metadata(document: &str) -> Option<(String, String)> {
    let mut lines = document.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return None;
    }
    let value: Frontmatter = serde_yaml_ng::from_str(&yaml).ok()?;
    if value.metadata.is_some_and(|m| m.internal) {
        return None;
    }
    let name = value.name.0;
    let description = value.description.0;
    if name.is_empty() || description.is_empty() {
        return None;
    }
    Some((sanitize(&name), sanitize(&description)))
}

fn sanitize(value: &str) -> String {
    let mut chars = value.chars().peekable();
    let mut output = String::new();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' || c == '\r' {
            if !output.ends_with(' ') {
                output.push(' ');
            }
        } else if !c.is_control() || c == '\t' {
            output.push(c);
        }
    }
    output.trim().to_owned()
}

fn local_path(base: &str, path: &str) -> Option<String> {
    if !path.starts_with("./") || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn plugin(
    plugin: &serde_json::Value,
    base: &str,
    dirs: &mut Vec<String>,
    groups: &mut BTreeMap<String, String>,
) {
    if let Some(skills) = plugin.get("skills").and_then(|s| s.as_array()) {
        for path in skills.iter().filter_map(|s| s.as_str()) {
            if let Some(path) = local_path(base, path) {
                dirs.push(path.rsplit_once('/').map_or("", |p| p.0).to_owned());
                if let Some(name) = plugin
                    .get("name")
                    .and_then(|s| s.as_str())
                    .filter(|s| !s.is_empty())
                {
                    groups.insert(path, name.to_owned());
                }
            }
        }
    }
    dirs.push(if base.is_empty() {
        "skills".into()
    } else {
        format!("{base}/skills")
    });
}

/// Documents are blobs from one immutable tree. Symlink policy remains with
/// the caller; manifests cannot cause external reads or remote plugin fetches.
pub fn discover_repository(
    paths: &[String],
    read: &mut dyn FnMut(&str) -> Option<String>,
) -> Vec<RepositorySkill> {
    let mut documents = BTreeMap::new();
    for path in [
        ".claude-plugin/marketplace.json",
        ".claude-plugin/plugin.json",
        "skills-lock.json",
    ] {
        if paths.iter().any(|p| p == path) {
            if let Some(document) = read(path) {
                documents.insert(path.to_owned(), document);
            }
        }
    }
    let mut groups = BTreeMap::new();
    let mut plugin_dirs = Vec::new();
    if let Some(marketplace) = documents
        .get(".claude-plugin/marketplace.json")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    {
        let root = marketplace
            .pointer("/metadata/pluginRoot")
            .and_then(|v| v.as_str());
        if let Some(plugins) = marketplace.get("plugins").and_then(|v| v.as_array()) {
            for entry in plugins {
                let source = match entry.get("source") {
                    None => "./",
                    Some(serde_json::Value::String(s)) => s,
                    _ => continue,
                };
                let Some(base) = root.map_or(Some(String::new()), |s| local_path("", s)) else {
                    continue;
                };
                let Some(base) = local_path(&base, source) else {
                    continue;
                };
                plugin(entry, &base, &mut plugin_dirs, &mut groups);
            }
        }
    }
    if let Some(single) = documents
        .get(".claude-plugin/plugin.json")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    {
        plugin(&single, "", &mut plugin_dirs, &mut groups);
    }
    let locked: BTreeSet<String> = documents
        .get("skills-lock.json")
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("skills").and_then(|s| s.as_object()).cloned())
        .map(|m| m.keys().map(|s| normalize_name(s)).collect())
        .unwrap_or_default();
    let mut candidates = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for path in paths {
        let dir = if path == "SKILL.md" {
            ""
        } else if let Some(dir) = path.strip_suffix("/SKILL.md") {
            dir
        } else {
            continue;
        };
        let mut parent = dir;
        while !parent.is_empty() {
            directories.insert(parent.to_owned());
            parent = parent.rsplit_once('/').map_or("", |p| p.0);
        }
        candidates.insert(dir.to_owned());
    }
    let mut scan = Scan {
        read,
        candidates,
        directories,
        groups,
        locked,
        parsed: BTreeSet::new(),
        names: BTreeSet::new(),
        skills: Vec::new(),
    };
    scan.add("");
    if !scan.skills.is_empty() {
        return scan.skills;
    }
    scan.walk("", 1, 1);
    for dir in [
        "skills",
        "skills/.curated",
        "skills/.experimental",
        "skills/.system",
    ]
    .into_iter()
    .chain(AGENT_DIRS.iter().copied())
    {
        scan.walk(dir, 1, 3);
    }
    for dir in plugin_dirs {
        scan.walk(
            &dir,
            1,
            if dir == "skills"
                || AGENT_DIRS.contains(&dir.as_str())
                || ["skills/.curated", "skills/.experimental", "skills/.system"]
                    .contains(&dir.as_str())
            {
                3
            } else {
                1
            },
        );
    }
    if scan.skills.is_empty() {
        let paths: Vec<_> = scan
            .candidates
            .iter()
            .filter(|path| {
                path.split('/').count() <= 5 && !path.split('/').any(|p| SKIP.contains(&p))
            })
            .cloned()
            .collect();
        for path in paths {
            scan.add(&path);
        }
    }
    scan.skills
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

struct Scan<'a> {
    read: &'a mut dyn FnMut(&str) -> Option<String>,
    candidates: BTreeSet<String>,
    directories: BTreeSet<String>,
    groups: BTreeMap<String, String>,
    locked: BTreeSet<String>,
    parsed: BTreeSet<String>,
    names: BTreeSet<String>,
    skills: Vec<RepositorySkill>,
}
impl Scan<'_> {
    fn add(&mut self, path: &str) -> bool {
        if !self.candidates.contains(path) {
            return false;
        }
        if !self.parsed.insert(path.to_owned()) {
            return true;
        }
        let document_path = if path.is_empty() {
            "SKILL.md".into()
        } else {
            format!("{path}/SKILL.md")
        };
        let Some((name, description)) =
            (self.read)(&document_path).and_then(|document| metadata(&document))
        else {
            return true;
        };
        if AGENT_DIRS
            .iter()
            .any(|dir| path == *dir || path.starts_with(&format!("{dir}/")))
            && (self.locked.contains(&normalize_name(&name))
                || self
                    .locked
                    .contains(&normalize_name(path.rsplit('/').next().unwrap_or_default())))
        {
            return true;
        }
        if self.names.insert(name.clone()) {
            self.skills.push(RepositorySkill {
                path: path.to_owned(),
                name: name.clone(),
                description: description.clone(),
                plugin_name: self.groups.get(path).cloned(),
            });
        }
        true
    }
    fn walk(&mut self, dir: &str, depth: usize, max: usize) {
        let children: Vec<_> = self
            .directories
            .iter()
            .filter(|p| p.rsplit_once('/').map_or("", |p| p.0) == dir)
            .cloned()
            .collect();
        for child in children {
            if self.add(&child)
                || depth >= max
                || SKIP.contains(&child.rsplit('/').next().unwrap_or_default())
            {
                continue;
            }
            self.walk(&child, depth + 1, max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unused_yaml_aliases_are_skipped_without_materializing_their_values() {
        let document = format!(
            "---\nname: safe\ndescription: safe\nunused: &large {}\nreferences: [{}]\n---\n",
            "x".repeat(200_000),
            vec!["*large"; 10_000].join(",")
        );
        assert!(document.len() < 512 * 1024);
        assert_eq!(metadata(&document), Some(("safe".into(), "safe".into())));
    }
    #[test]
    fn excluded_documents_are_never_read() {
        let paths = vec![
            "skills/valid/SKILL.md".into(),
            "node_modules/pkg/SKILL.md".into(),
            "examples/ignored/SKILL.md".into(),
        ];
        let result = discover_repository(&paths, &mut |path| {
            assert_eq!(path, "skills/valid/SKILL.md");
            Some(doc("valid"))
        });
        assert_eq!(result.len(), 1);
    }
    fn discover(documents: &BTreeMap<String, String>) -> Vec<RepositorySkill> {
        discover_repository(
            &documents.keys().cloned().collect::<Vec<_>>(),
            &mut |path| documents.get(path).cloned(),
        )
    }
    fn doc(name: &str) -> String {
        format!("---\nname: {name}\ndescription: A skill\n---\n")
    }
    fn paths(result: Vec<RepositorySkill>) -> Vec<String> {
        result.into_iter().map(|s| s.path).collect()
    }

    #[test]
    fn nested_skill_is_not_a_separate_member_and_names_are_deduplicated() {
        let documents = BTreeMap::from([
            ("skill/SKILL.md".into(), doc("mmx-cli")),
            ("skill/h3-video/SKILL.md".into(), doc("mmx-h3-video")),
        ]);
        assert_eq!(paths(discover(&documents)), vec!["skill"]);
        let documents = BTreeMap::from([
            ("skills/kami/SKILL.md".into(), doc("kami")),
            ("plugins/kami/skills/kami/SKILL.md".into(), doc("kami")),
            (
                ".claude-plugin/plugin.json".into(),
                r#"{"name":"kami","skills":["./plugins/kami/skills/kami"]}"#.into(),
            ),
        ]);
        assert_eq!(paths(discover(&documents)), vec!["skills/kami"]);
    }
    #[test]
    fn plugin_manifest_groups_exact_paths_and_other_members_stay_ungrouped() {
        let documents = BTreeMap::from([
            ("skills/engineering/code-review/SKILL.md".into(), doc("code-review")),
            ("skills/misc/retro/SKILL.md".into(), doc("retro")),
            (".claude-plugin/plugin.json".into(), r#"{"name":"mattpocock-skills","skills":["./skills/engineering/code-review","./../../escape"]}"#.into()),
        ]);
        let result = discover(&documents);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].plugin_name.as_deref(), Some("mattpocock-skills"));
        assert_eq!(result[1].plugin_name, None);
    }
    #[test]
    fn yaml_validation_internal_and_locked_dependencies_match_default_cli() {
        let documents = BTreeMap::from([
            (
                "skills/valid/SKILL.md".into(),
                "---\nname: 'real'\ndescription: >-\n  first line\n  second line\n---\n".into(),
            ),
            (
                "skills/internal/SKILL.md".into(),
                "---\nname: hidden\ndescription: Hidden\nmetadata:\n  internal: true\n---\n".into(),
            ),
            (
                "skills/invalid/SKILL.md".into(),
                "---\nname: 123\ndescription: bad\n---\n".into(),
            ),
            (".agents/skills/installed/SKILL.md".into(), doc("installed")),
            (
                "skills-lock.json".into(),
                r#"{"version":1,"skills":{"installed":{}}}"#.into(),
            ),
        ]);
        let result = discover(&documents);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].description, "first line second line");
    }
    #[test]
    fn containers_are_three_levels_deep_and_fallback_is_bounded() {
        let documents = BTreeMap::from([
            ("skills/a/b/c/SKILL.md".into(), doc("deep")),
            ("skills/a/b/d/e/SKILL.md".into(), doc("too-deep")),
            ("examples/unrelated/SKILL.md".into(), doc("unrelated")),
        ]);
        assert_eq!(paths(discover(&documents)), vec!["skills/a/b/c"]);
        let documents = BTreeMap::from([
            ("examples/a/b/c/d/SKILL.md".into(), doc("fallback")),
            ("examples/a/b/c/d/e/SKILL.md".into(), doc("too-deep")),
            ("node_modules/package/SKILL.md".into(), doc("dependency")),
        ]);
        assert_eq!(paths(discover(&documents)), vec!["examples/a/b/c/d"]);
    }
}
