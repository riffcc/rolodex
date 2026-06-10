use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use serde::de::Error as _;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

const PROJECT_NAVIGATION_STATE_FILE: &str = "project-navigation.json";
const MAX_RECENT_PROJECTS: usize = 48;
const DEFAULT_ATTENTION_IDLE_DELAY_SECONDS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectTabDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectWorkspaceDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewTabPlacement {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttentionMode {
    Off,
    #[default]
    Soft,
    On,
}

impl AttentionMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Soft => "soft",
            Self::On => "on",
        }
    }

    pub(crate) fn shows_markers(self) -> bool {
        self != Self::Off
    }

    pub(crate) fn auto_switches(self) -> bool {
        self == Self::On
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Soft,
            Self::Soft => Self::On,
            Self::On => Self::Off,
        }
    }
}

impl<'de> Deserialize<'de> for AttentionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bool(bool),
            String(String),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Bool(false) => Ok(AttentionMode::Soft),
            Repr::Bool(true) => Ok(AttentionMode::On),
            Repr::String(value) => match value.as_str() {
                "off" => Ok(AttentionMode::Off),
                "soft" => Ok(AttentionMode::Soft),
                "on" => Ok(AttentionMode::On),
                _ => Err(D::Error::custom(format!(
                    "invalid attention mode `{value}`; expected `off`, `soft`, or `on`"
                ))),
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedProjectNavigationState {
    #[serde(default)]
    pub(crate) favorites: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) recents: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) favorite_descriptions: HashMap<PathBuf, String>,
    #[serde(default)]
    pub(crate) workspace_names: Vec<Option<String>>,
    #[serde(default, alias = "attention_mode_enabled")]
    pub(crate) attention_mode: AttentionMode,
    #[serde(default = "default_attention_idle_delay_seconds")]
    pub(crate) attention_idle_delay_seconds: u64,
    #[serde(default)]
    pub(crate) saved_workspace_session: Option<PersistedWorkspaceSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedWorkspaceSession {
    #[serde(default)]
    pub(crate) workspaces: Vec<PersistedWorkspace>,
    #[serde(default)]
    pub(crate) active_workspace: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedWorkspace {
    #[serde(default)]
    pub(crate) tabs: Vec<PersistedWorkspaceTab>,
    #[serde(default)]
    pub(crate) active_tab: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedWorkspaceTab {
    pub(crate) thread_id: ThreadId,
    pub(crate) cwd: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RestoredWorkspace {
    pub(crate) tabs: Vec<(ThreadId, PathBuf)>,
    pub(crate) active_tab: Option<usize>,
}

impl PersistedWorkspaceSession {
    pub(crate) fn startup_tab(&self) -> Option<&PersistedWorkspaceTab> {
        self.workspaces
            .get(self.active_workspace)
            .and_then(PersistedWorkspace::active_tab_entry)
            .or_else(|| {
                self.workspaces.iter().find_map(|workspace| {
                    workspace
                        .active_tab_entry()
                        .or_else(|| workspace.tabs.first())
                })
            })
    }
}

impl PersistedWorkspace {
    fn active_tab_entry(&self) -> Option<&PersistedWorkspaceTab> {
        self.active_tab.and_then(|idx| self.tabs.get(idx))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProjectWorkspace {
    tabs: Vec<ThreadId>,
    active_tab: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectNavigationState {
    persisted: PersistedProjectNavigationState,
    workspaces: Vec<ProjectWorkspace>,
    active_workspace: usize,
    tab_cwds: HashMap<ThreadId, PathBuf>,
}

impl Default for ProjectNavigationState {
    fn default() -> Self {
        Self {
            persisted: PersistedProjectNavigationState::default(),
            workspaces: vec![ProjectWorkspace::default()],
            active_workspace: 0,
            tab_cwds: HashMap::new(),
        }
    }
}

impl ProjectNavigationState {
    pub(crate) fn load(codex_home: &Path) -> Self {
        let persisted = Self::load_persisted_state(codex_home).unwrap_or_default();
        Self {
            persisted,
            ..Self::default()
        }
    }

    pub(crate) fn load_saved_workspace_session(
        codex_home: &Path,
    ) -> io::Result<Option<PersistedWorkspaceSession>> {
        Self::load_persisted_state(codex_home).map(|state| state.saved_workspace_session)
    }

    pub(crate) fn save(&self, codex_home: &Path) -> io::Result<()> {
        let path = Self::state_file_path(codex_home);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.persisted)
            .map_err(|err| io::Error::other(format!("failed to serialize project state: {err}")))?;
        fs::write(path, json)
    }

    pub(crate) fn state_file_path(codex_home: &Path) -> PathBuf {
        codex_home.join(PROJECT_NAVIGATION_STATE_FILE)
    }

    pub(crate) fn reload_persisted(&mut self, codex_home: &Path) -> io::Result<()> {
        self.persisted = Self::load_persisted_state(codex_home)?;
        Ok(())
    }

    pub(crate) fn active_workspace_index(&self) -> usize {
        self.active_workspace
    }

    pub(crate) fn workspace_count(&self) -> usize {
        self.workspaces.len()
    }

    #[cfg(test)]
    pub(crate) fn active_workspace_is_empty(&self) -> bool {
        self.workspaces
            .get(self.active_workspace)
            .is_none_or(|workspace| workspace.tabs.is_empty())
    }

    pub(crate) fn workspace_name(&self, index: usize) -> String {
        self.persisted
            .workspace_names
            .get(index)
            .and_then(|name| name.as_deref())
            .filter(|name| !name.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Workspace {}", index + 1))
    }

    pub(crate) fn custom_workspace_name(&self, index: usize) -> Option<&str> {
        self.persisted
            .workspace_names
            .get(index)
            .and_then(|name| name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    pub(crate) fn favorites(&self) -> &[PathBuf] {
        &self.persisted.favorites
    }

    pub(crate) fn recents(&self) -> &[PathBuf] {
        &self.persisted.recents
    }

    pub(crate) fn is_favorite(&self, path: &Path) -> bool {
        self.persisted
            .favorites
            .iter()
            .any(|favorite| favorite == path)
    }

    pub(crate) fn add_favorite(&mut self, path: PathBuf) {
        if self.is_favorite(&path) {
            return;
        }
        self.persisted.favorites.insert(0, path);
    }

    pub(crate) fn remove_favorite(&mut self, path: &Path) {
        self.persisted.favorites.retain(|favorite| favorite != path);
        self.persisted.favorite_descriptions.remove(path);
    }

    pub(crate) fn toggle_favorite(&mut self, path: PathBuf) -> bool {
        if self.is_favorite(&path) {
            self.remove_favorite(&path);
            false
        } else {
            self.add_favorite(path);
            true
        }
    }

    pub(crate) fn record_recent(&mut self, path: PathBuf) {
        self.persisted.recents.retain(|recent| recent != &path);
        self.persisted.recents.insert(0, path);
        self.persisted.recents.truncate(MAX_RECENT_PROJECTS);
    }

    pub(crate) fn remove_recent(&mut self, path: &Path) {
        self.persisted.recents.retain(|recent| recent != path);
    }

    pub(crate) fn favorite_description(&self, path: &Path) -> Option<&str> {
        self.persisted
            .favorite_descriptions
            .get(path)
            .map(String::as_str)
    }

    pub(crate) fn set_favorite_description(&mut self, path: PathBuf, description: Option<String>) {
        let description = description.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        match description {
            Some(description) => {
                self.persisted
                    .favorite_descriptions
                    .insert(path, description);
            }
            None => {
                self.persisted.favorite_descriptions.remove(&path);
            }
        }
    }

    pub(crate) fn set_workspace_name(&mut self, index: usize, name: Option<String>) {
        if self.persisted.workspace_names.len() <= index {
            self.persisted.workspace_names.resize(index + 1, None);
        }
        self.persisted.workspace_names[index] = name.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
    }

    pub(crate) fn attention_mode(&self) -> AttentionMode {
        self.persisted.attention_mode
    }

    pub(crate) fn set_attention_mode(&mut self, mode: AttentionMode) {
        self.persisted.attention_mode = mode;
    }

    pub(crate) fn attention_markers_enabled(&self) -> bool {
        self.persisted.attention_mode.shows_markers()
    }

    pub(crate) fn attention_auto_switches(&self) -> bool {
        self.persisted.attention_mode.auto_switches()
    }

    pub(crate) fn attention_idle_delay_seconds(&self) -> u64 {
        self.persisted.attention_idle_delay_seconds
    }

    pub(crate) fn saved_workspace_session(&self) -> Option<&PersistedWorkspaceSession> {
        self.persisted.saved_workspace_session.as_ref()
    }

    pub(crate) fn save_current_workspace_session(&mut self) -> bool {
        self.persisted.saved_workspace_session = self.capture_workspace_session();
        self.persisted.saved_workspace_session.is_some()
    }

    pub(crate) fn apply_restored_workspace_session(
        &mut self,
        workspaces: Vec<RestoredWorkspace>,
        active_workspace: usize,
    ) {
        let mut tab_cwds = HashMap::new();
        let mut restored_workspaces = Vec::new();

        for workspace in workspaces {
            let tabs: Vec<ThreadId> = workspace
                .tabs
                .into_iter()
                .map(|(thread_id, cwd)| {
                    tab_cwds.insert(thread_id, cwd);
                    thread_id
                })
                .collect();
            let active_tab = workspace.active_tab.filter(|idx| *idx < tabs.len());
            restored_workspaces.push(ProjectWorkspace { tabs, active_tab });
        }

        if restored_workspaces.is_empty() {
            restored_workspaces.push(ProjectWorkspace::default());
        }

        self.tab_cwds = tab_cwds;
        self.workspaces = restored_workspaces;
        self.active_workspace = active_workspace.min(self.workspaces.len().saturating_sub(1));
    }

    #[cfg(test)]
    pub(crate) fn persisted_state(&self) -> &PersistedProjectNavigationState {
        &self.persisted
    }

    pub(crate) fn remember_project_thread(&mut self, thread_id: ThreadId, cwd: PathBuf) {
        self.tab_cwds.insert(thread_id, cwd);
        self.ensure_thread_in_active_workspace(thread_id);
    }

    pub(crate) fn insert_project_thread(
        &mut self,
        thread_id: ThreadId,
        cwd: PathBuf,
        anchor_thread_id: Option<ThreadId>,
        placement: NewTabPlacement,
    ) {
        self.remove_thread(thread_id);
        self.tab_cwds.insert(thread_id, cwd);
        let Some(workspace) = self.workspaces.get_mut(self.active_workspace) else {
            return;
        };
        let insert_idx = anchor_thread_id
            .and_then(|anchor| workspace.tabs.iter().position(|thread| *thread == anchor))
            .map(|idx| match placement {
                NewTabPlacement::Left => idx,
                NewTabPlacement::Right => idx + 1,
            })
            .unwrap_or(workspace.tabs.len());
        workspace.tabs.insert(insert_idx, thread_id);
        workspace.active_tab = Some(insert_idx);
    }

    pub(crate) fn remove_thread(&mut self, thread_id: ThreadId) {
        self.tab_cwds.remove(&thread_id);
        for workspace in &mut self.workspaces {
            if let Some(idx) = workspace
                .tabs
                .iter()
                .position(|thread| *thread == thread_id)
            {
                workspace.tabs.remove(idx);
                workspace.active_tab = match workspace.active_tab {
                    Some(_active_idx) if workspace.tabs.is_empty() => None,
                    Some(active_idx) if active_idx > idx => Some(active_idx - 1),
                    Some(active_idx) if active_idx >= workspace.tabs.len() => {
                        Some(workspace.tabs.len().saturating_sub(1))
                    }
                    other => other,
                };
            }
        }
    }

    pub(crate) fn set_active_thread(&mut self, thread_id: ThreadId) {
        if let Some((workspace_idx, tab_idx)) = self.find_tab(thread_id) {
            self.active_workspace = workspace_idx;
            if let Some(workspace) = self.workspaces.get_mut(workspace_idx) {
                workspace.active_tab = Some(tab_idx);
            }
        }
    }

    pub(crate) fn adjacent_tab(
        &mut self,
        current_thread_id: Option<ThreadId>,
        direction: ProjectTabDirection,
    ) -> Option<ThreadId> {
        let workspace = self.workspaces.get_mut(self.active_workspace)?;
        if workspace.tabs.len() < 2 {
            return None;
        }
        let current_idx = current_thread_id
            .and_then(|thread_id| {
                workspace
                    .tabs
                    .iter()
                    .position(|thread| *thread == thread_id)
            })
            .or(workspace.active_tab)
            .unwrap_or(0);
        let next_idx = match direction {
            ProjectTabDirection::Previous => {
                if current_idx == 0 {
                    workspace.tabs.len() - 1
                } else {
                    current_idx - 1
                }
            }
            ProjectTabDirection::Next => (current_idx + 1) % workspace.tabs.len(),
        };
        workspace.active_tab = Some(next_idx);
        workspace.tabs.get(next_idx).copied()
    }

    pub(crate) fn switch_workspace(
        &mut self,
        direction: ProjectWorkspaceDirection,
    ) -> Option<ThreadId> {
        match direction {
            ProjectWorkspaceDirection::Previous => {
                if self.active_workspace == 0 {
                    self.active_workspace = self.workspaces.len().saturating_sub(1);
                } else {
                    self.active_workspace -= 1;
                }
            }
            ProjectWorkspaceDirection::Next => {
                if self.active_workspace + 1 >= self.workspaces.len() {
                    self.workspaces.push(ProjectWorkspace::default());
                }
                self.active_workspace += 1;
            }
        }
        self.workspaces
            .get(self.active_workspace)
            .and_then(|workspace| {
                workspace
                    .active_tab
                    .and_then(|idx| workspace.tabs.get(idx).copied())
            })
            .or_else(|| {
                self.workspaces
                    .get(self.active_workspace)
                    .and_then(|workspace| workspace.tabs.first().copied())
            })
    }

    pub(crate) fn active_workspace_tabs(&self) -> Vec<(ThreadId, PathBuf)> {
        self.workspace_tabs(self.active_workspace)
    }

    pub(crate) fn all_workspace_tabs(&self) -> Vec<(usize, ThreadId, PathBuf)> {
        self.workspaces
            .iter()
            .enumerate()
            .flat_map(|(workspace_idx, workspace)| {
                workspace.tabs.iter().filter_map(move |thread_id| {
                    self.tab_cwds
                        .get(thread_id)
                        .cloned()
                        .map(|cwd| (workspace_idx, *thread_id, cwd))
                })
            })
            .collect()
    }

    pub(crate) fn workspace_tabs(&self, workspace_idx: usize) -> Vec<(ThreadId, PathBuf)> {
        self.workspaces
            .get(workspace_idx)
            .map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .filter_map(|thread_id| {
                        self.tab_cwds
                            .get(thread_id)
                            .cloned()
                            .map(|cwd| (*thread_id, cwd))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn tab_cwd(&self, thread_id: ThreadId) -> Option<&Path> {
        self.tab_cwds.get(&thread_id).map(PathBuf::as_path)
    }

    pub(crate) fn find_thread_for_cwd(&self, cwd: &Path) -> Option<(usize, ThreadId)> {
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(workspace_idx, workspace)| {
                workspace.tabs.iter().find_map(|thread_id| {
                    self.tab_cwds
                        .get(thread_id)
                        .is_some_and(|tab_cwd| tab_cwd == cwd)
                        .then_some((workspace_idx, *thread_id))
                })
            })
    }

    fn ensure_thread_in_active_workspace(&mut self, thread_id: ThreadId) {
        if self.find_tab(thread_id).is_some() {
            return;
        }
        if self.workspaces.is_empty() {
            self.workspaces.push(ProjectWorkspace::default());
            self.active_workspace = 0;
        }
        let Some(workspace) = self.workspaces.get_mut(self.active_workspace) else {
            return;
        };
        workspace.tabs.push(thread_id);
        workspace.active_tab = Some(workspace.tabs.len().saturating_sub(1));
    }

    fn find_tab(&self, thread_id: ThreadId) -> Option<(usize, usize)> {
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(workspace_idx, workspace)| {
                workspace
                    .tabs
                    .iter()
                    .position(|thread| *thread == thread_id)
                    .map(|tab_idx| (workspace_idx, tab_idx))
            })
    }

    fn load_persisted_state(codex_home: &Path) -> io::Result<PersistedProjectNavigationState> {
        let path = Self::state_file_path(codex_home);
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(PersistedProjectNavigationState::default());
            }
            Err(err) => return Err(err),
        };
        serde_json::from_str(&raw).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse project state: {err}"),
            )
        })
    }

    fn capture_workspace_session(&self) -> Option<PersistedWorkspaceSession> {
        let workspaces: Vec<PersistedWorkspace> = self
            .workspaces
            .iter()
            .map(|workspace| PersistedWorkspace {
                tabs: workspace
                    .tabs
                    .iter()
                    .filter_map(|thread_id| {
                        self.tab_cwds
                            .get(thread_id)
                            .cloned()
                            .map(|cwd| PersistedWorkspaceTab {
                                thread_id: *thread_id,
                                cwd,
                            })
                    })
                    .collect(),
                active_tab: workspace.active_tab,
            })
            .collect();

        workspaces
            .iter()
            .any(|workspace| !workspace.tabs.is_empty())
            .then_some(PersistedWorkspaceSession {
                workspaces,
                active_workspace: self.active_workspace,
            })
    }
}

fn default_attention_idle_delay_seconds() -> u64 {
    DEFAULT_ATTENTION_IDLE_DELAY_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn recents_are_deduped_and_most_recent_first() {
        let mut state = ProjectNavigationState::default();
        state.record_recent(PathBuf::from("/work/a"));
        state.record_recent(PathBuf::from("/work/b"));
        state.record_recent(PathBuf::from("/work/a"));

        assert_eq!(
            state.recents(),
            &[PathBuf::from("/work/a"), PathBuf::from("/work/b")]
        );
    }

    #[test]
    fn inserts_new_tabs_adjacent_to_anchor() {
        let mut state = ProjectNavigationState::default();
        let first = ThreadId::new();
        let second = ThreadId::new();
        let third = ThreadId::new();

        state.remember_project_thread(first, PathBuf::from("/work/first"));
        state.insert_project_thread(
            second,
            PathBuf::from("/work/second"),
            Some(first),
            NewTabPlacement::Right,
        );
        state.insert_project_thread(
            third,
            PathBuf::from("/work/third"),
            Some(first),
            NewTabPlacement::Left,
        );

        assert_eq!(
            state.active_workspace_tabs(),
            vec![
                (third, PathBuf::from("/work/third")),
                (first, PathBuf::from("/work/first")),
                (second, PathBuf::from("/work/second")),
            ]
        );
    }

    #[test]
    fn next_workspace_creates_a_new_empty_workspace() {
        let mut state = ProjectNavigationState::default();
        let first = ThreadId::new();
        state.remember_project_thread(first, PathBuf::from("/work/first"));

        let switched = state.switch_workspace(ProjectWorkspaceDirection::Next);

        assert_eq!(switched, None);
        assert_eq!(state.active_workspace_index(), 1);
        assert_eq!(state.workspace_count(), 2);
        assert_eq!(state.active_workspace_is_empty(), true);
    }

    #[test]
    fn save_and_load_round_trip_persisted_state() {
        let codex_home = tempdir().expect("create temp codex home");
        let mut state = ProjectNavigationState::default();
        state.add_favorite(PathBuf::from("/favorites/codex"));
        state.record_recent(PathBuf::from("/recent/codex"));
        state.set_attention_mode(AttentionMode::On);

        state
            .save(codex_home.path())
            .expect("save project navigation state");

        let loaded = ProjectNavigationState::load(codex_home.path());
        assert_eq!(loaded.favorites(), &[PathBuf::from("/favorites/codex")]);
        assert_eq!(loaded.recents(), &[PathBuf::from("/recent/codex")]);
        assert_eq!(loaded.attention_mode(), AttentionMode::On);
    }

    #[test]
    fn reload_persisted_preserves_open_tabs() {
        let codex_home = tempdir().expect("create temp codex home");
        let thread_id = ThreadId::new();
        let mut state = ProjectNavigationState::default();
        state.remember_project_thread(thread_id, PathBuf::from("/work/codex"));
        state.add_favorite(PathBuf::from("/favorites/original"));
        state
            .save(codex_home.path())
            .expect("save project navigation state");

        fs::write(
            ProjectNavigationState::state_file_path(codex_home.path()),
            r#"{
  "favorites": ["/favorites/edited"],
  "recents": ["/recent/edited"]
}"#,
        )
        .expect("rewrite project navigation state");

        state
            .reload_persisted(codex_home.path())
            .expect("reload project navigation state");

        assert_eq!(
            state.active_workspace_tabs(),
            vec![(thread_id, PathBuf::from("/work/codex"))]
        );
        assert_eq!(state.favorites(), &[PathBuf::from("/favorites/edited")]);
        assert_eq!(state.recents(), &[PathBuf::from("/recent/edited")]);
    }

    #[test]
    fn remove_recent_forgets_matching_path() {
        let mut state = ProjectNavigationState::default();
        state.record_recent(PathBuf::from("/work/a"));
        state.record_recent(PathBuf::from("/work/b"));

        state.remove_recent(Path::new("/work/a"));

        assert_eq!(state.recents(), &[PathBuf::from("/work/b")]);
    }

    #[test]
    fn save_current_workspace_session_round_trips() {
        let codex_home = tempdir().expect("create temp codex home");
        let first = ThreadId::new();
        let second = ThreadId::new();
        let third = ThreadId::new();
        let mut state = ProjectNavigationState::default();

        state.remember_project_thread(first, PathBuf::from("/work/first"));
        state.insert_project_thread(
            second,
            PathBuf::from("/work/second"),
            Some(first),
            NewTabPlacement::Right,
        );
        state.switch_workspace(ProjectWorkspaceDirection::Next);
        state.remember_project_thread(third, PathBuf::from("/work/third"));
        state.switch_workspace(ProjectWorkspaceDirection::Previous);

        assert!(state.save_current_workspace_session());
        state
            .save(codex_home.path())
            .expect("save project navigation state");

        let restored = ProjectNavigationState::load_saved_workspace_session(codex_home.path())
            .expect("load saved workspace session")
            .expect("workspace session should exist");

        assert_eq!(restored.active_workspace, 0);
        assert_eq!(restored.workspaces.len(), 2);
        assert_eq!(
            restored.workspaces[0].tabs,
            vec![
                PersistedWorkspaceTab {
                    thread_id: first,
                    cwd: PathBuf::from("/work/first"),
                },
                PersistedWorkspaceTab {
                    thread_id: second,
                    cwd: PathBuf::from("/work/second"),
                },
            ]
        );
        assert_eq!(
            restored.workspaces[1].tabs,
            vec![PersistedWorkspaceTab {
                thread_id: third,
                cwd: PathBuf::from("/work/third"),
            }]
        );
    }

    #[test]
    fn apply_restored_workspace_session_replaces_open_tabs() {
        let first = ThreadId::new();
        let second = ThreadId::new();
        let mut state = ProjectNavigationState::default();
        state.remember_project_thread(ThreadId::new(), PathBuf::from("/stale"));

        state.apply_restored_workspace_session(
            vec![
                RestoredWorkspace {
                    tabs: vec![(first, PathBuf::from("/work/first"))],
                    active_tab: Some(0),
                },
                RestoredWorkspace {
                    tabs: vec![(second, PathBuf::from("/work/second"))],
                    active_tab: Some(0),
                },
            ],
            1,
        );

        assert_eq!(state.active_workspace_index(), 1);
        assert_eq!(
            state.workspace_tabs(0),
            vec![(first, PathBuf::from("/work/first"))]
        );
        assert_eq!(
            state.workspace_tabs(1),
            vec![(second, PathBuf::from("/work/second"))]
        );
    }

    #[test]
    fn load_legacy_boolean_attention_mode_as_soft_or_on() {
        let codex_home = tempdir().expect("create temp codex home");
        fs::write(
            ProjectNavigationState::state_file_path(codex_home.path()),
            r#"{
  "attention_mode_enabled": false
}"#,
        )
        .expect("write legacy disabled attention mode");

        let loaded = ProjectNavigationState::load(codex_home.path());
        assert_eq!(loaded.attention_mode(), AttentionMode::Soft);

        fs::write(
            ProjectNavigationState::state_file_path(codex_home.path()),
            r#"{
  "attention_mode_enabled": true
}"#,
        )
        .expect("write legacy enabled attention mode");

        let loaded = ProjectNavigationState::load(codex_home.path());
        assert_eq!(loaded.attention_mode(), AttentionMode::On);
    }
}
