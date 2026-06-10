use super::*;
use crate::app_event::ProjectOpenTarget;
use crate::app_event::ProjectTabPlacement;
use crate::app_event::SplitAxis;
use crate::app_event::SplitPaneTarget;
use crate::bottom_pane::FavoriteProjectTile;
use crate::bottom_pane::FavoritesEditorView;
use crate::bottom_pane::ProjectChooserView;
use crate::bottom_pane::ProjectSwitcherTabTile;
use crate::bottom_pane::ProjectSwitcherView;
use crate::chatwidget::ProjectTabChromeEntry;
use crate::chatwidget::ProjectTabsChromeState;
use project_navigation::NewTabPlacement;
use project_navigation::ProjectTabDirection;
use project_navigation::ProjectWorkspaceDirection;

impl App {
    fn persist_project_navigation_state(&mut self, action: &str) {
        if let Err(err) = self.project_navigation.save(&self.config.codex_home) {
            tracing::warn!(error = %err, action, "failed to persist project navigation state");
            self.chat_widget
                .add_error_message(format!("Failed to save project navigation: {err}"));
        }
    }

    pub(super) fn project_label(path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }

    fn build_project_switcher_tabs(&self) -> Vec<ProjectSwitcherTabTile> {
        self.project_navigation
            .active_workspace_tabs()
            .into_iter()
            .map(|(thread_id, cwd)| ProjectSwitcherTabTile {
                thread_id,
                label: Self::project_label(&cwd),
                cwd,
                summary: None,
                is_active: self.active_thread_id == Some(thread_id),
                attention: None,
            })
            .collect()
    }

    fn build_favorite_tiles(&self) -> Vec<FavoriteProjectTile> {
        self.project_navigation
            .favorites()
            .iter()
            .map(|cwd| FavoriteProjectTile {
                cwd: cwd.clone(),
                label: Self::project_label(cwd),
                description: self
                    .project_navigation
                    .favorite_description(cwd)
                    .map(ToOwned::to_owned),
                is_open: self.project_navigation.find_thread_for_cwd(cwd).is_some(),
            })
            .collect()
    }

    pub(super) fn sync_project_tabs_chrome(&mut self) {
        let tabs = self.build_project_switcher_tabs();
        if tabs.is_empty() {
            self.chat_widget.set_project_tabs(None);
            return;
        }
        self.chat_widget
            .set_project_tabs(Some(ProjectTabsChromeState {
                workspace_custom_name: self
                    .project_navigation
                    .custom_workspace_name(self.project_navigation.active_workspace_index())
                    .map(ToOwned::to_owned),
                workspace_index: self.project_navigation.active_workspace_index(),
                workspace_count: self.project_navigation.workspace_count(),
                attention_mode: self.project_navigation.attention_mode(),
                tabs: tabs
                    .into_iter()
                    .map(|tab| ProjectTabChromeEntry {
                        label: tab.label,
                        is_active: tab.is_active,
                        attention: tab.attention,
                    })
                    .collect(),
            }));
    }

    pub(super) fn open_project_navigator(&mut self) {
        let workspace_index = self.project_navigation.active_workspace_index();
        self.chat_widget
            .show_custom_view(Box::new(ProjectSwitcherView::new(
                self.app_event_tx.clone(),
                self.project_navigation.workspace_name(workspace_index),
                workspace_index,
                self.project_navigation.workspace_count(),
                self.project_navigation.attention_mode(),
                self.build_project_switcher_tabs(),
                self.build_favorite_tiles(),
                self.config.cwd.to_path_buf(),
            )));
    }

    pub(super) fn open_project_directory_browser(&mut self, root: PathBuf) {
        self.open_project_chooser(root, ProjectOpenTarget::Tab(ProjectTabPlacement::Right));
    }

    pub(super) fn open_project_chooser(&mut self, root: PathBuf, target: ProjectOpenTarget) {
        self.chat_widget
            .show_custom_view(Box::new(ProjectChooserView::new(
                self.app_event_tx.clone(),
                self.build_favorite_tiles(),
                self.project_navigation.recents().to_vec(),
                Some(root),
                target,
            )));
    }

    pub(super) fn open_split_pane_project_chooser(&mut self, axis: SplitAxis) {
        let Some(thread_id) = self.active_thread_id else {
            return;
        };
        let favorites = self.build_favorite_tiles();
        let recents = self.project_navigation.recents().to_vec();
        let cwd = self.config.cwd.to_path_buf();
        let split_pane = self
            .split_pane
            .get_or_insert_with(|| split_pane::SplitPaneState::new(thread_id));
        let pane_id = split_pane.active_pane_id();
        self.chat_widget
            .show_custom_view(Box::new(ProjectChooserView::new(
                self.app_event_tx.clone(),
                favorites,
                recents,
                Some(cwd),
                ProjectOpenTarget::SplitPane(SplitPaneTarget { pane_id, axis }),
            )));
    }

    pub(super) fn open_project_favorites_manager(&mut self, initial_path: Option<PathBuf>) {
        self.chat_widget
            .show_custom_view(Box::new(FavoritesEditorView::new(
                self.app_event_tx.clone(),
                self.build_favorite_tiles(),
                self.project_navigation.recents().to_vec(),
                initial_path,
            )));
    }

    pub(super) fn toggle_project_favorite(&mut self, cwd: PathBuf) {
        let added = self.project_navigation.toggle_favorite(cwd.clone());
        self.persist_project_navigation_state("updating favorites");
        self.chat_widget.add_info_message(
            format!(
                "{} favorite project {}.",
                if added { "Added" } else { "Removed" },
                cwd.display()
            ),
            None,
        );
    }

    pub(super) fn forget_recent_project(&mut self, cwd: PathBuf) {
        self.project_navigation.remove_recent(&cwd);
        self.persist_project_navigation_state("updating recent projects");
    }

    pub(super) async fn edit_project_favorites(&mut self, _tui: &mut tui::Tui) {
        self.open_project_favorites_manager(None);
    }

    pub(super) fn cycle_attention_mode(&mut self) {
        self.apply_attention_mode(self.project_navigation.attention_mode().next());
    }

    pub(super) fn apply_attention_mode(&mut self, mode: AttentionMode) {
        self.project_navigation.set_attention_mode(mode);
        self.persist_project_navigation_state("updating attention mode");
        self.sync_project_tabs_chrome();
        self.chat_widget
            .add_info_message(format!("Project attention mode: {}.", mode.label()), None);
    }

    pub(super) fn save_workspace_session(&mut self) -> bool {
        if !self.project_navigation.save_current_workspace_session() {
            self.chat_widget
                .add_error_message("No project tabs are open to save.".to_string());
            return false;
        }
        self.persist_project_navigation_state("saving workspace session");
        true
    }

    async fn activate_project_thread(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        cwd: PathBuf,
    ) -> Result<()> {
        let mut config = self.rebuild_config_for_cwd(cwd.clone()).await?;
        self.apply_runtime_policy_overrides(&mut config);
        self.config = config;
        self.select_agent_thread(tui, app_server, thread_id).await?;
        self.project_navigation.set_active_thread(thread_id);
        self.project_navigation.record_recent(cwd);
        self.persist_project_navigation_state("switching project tabs");
        self.sync_project_tabs_chrome();
        Ok(())
    }

    async fn start_project_thread(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        cwd: PathBuf,
        placement: ProjectTabPlacement,
    ) -> Result<ThreadId> {
        let mut config = self.rebuild_config_for_cwd(cwd.clone()).await?;
        self.apply_runtime_policy_overrides(&mut config);
        let started = app_server
            .start_thread_with_session_start_source(&config, None)
            .await?;
        let thread_id = started.session.thread_id;
        let channel = self.ensure_thread_channel(thread_id);
        channel
            .store
            .lock()
            .await
            .set_session(started.session, started.turns);
        self.project_navigation.insert_project_thread(
            thread_id,
            cwd.clone(),
            self.active_thread_id,
            match placement {
                ProjectTabPlacement::Left => NewTabPlacement::Left,
                ProjectTabPlacement::Right => NewTabPlacement::Right,
            },
        );
        self.config = config;
        self.upsert_agent_picker_thread(thread_id, None, None, false);
        self.select_agent_thread(tui, app_server, thread_id).await?;
        self.project_navigation.record_recent(cwd);
        self.persist_project_navigation_state("opening a project tab");
        self.sync_project_tabs_chrome();
        Ok(thread_id)
    }

    fn bind_split_pane_thread(
        &mut self,
        current_thread_id: ThreadId,
        target_thread_id: ThreadId,
        target: SplitPaneTarget,
    ) {
        let mut split_pane = self
            .split_pane
            .take()
            .unwrap_or_else(|| split_pane::SplitPaneState::new(current_thread_id));
        if let Some(pane_id) = split_pane.pane_for_thread(target_thread_id) {
            split_pane.focus_pane(pane_id);
        } else if split_pane.focus_pane(target.pane_id).is_some() {
            split_pane.split_active(target_thread_id, target.axis);
        }
        self.split_pane = (split_pane.leaf_count() > 1).then_some(split_pane);
    }

    pub(super) async fn focus_or_open_project(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        cwd: PathBuf,
    ) -> Result<()> {
        self.focus_or_open_project_at_target(
            tui,
            app_server,
            cwd,
            ProjectOpenTarget::Tab(ProjectTabPlacement::Right),
        )
        .await
    }

    pub(super) async fn focus_or_open_project_at_target(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        cwd: PathBuf,
        target: ProjectOpenTarget,
    ) -> Result<()> {
        if let Some((_, thread_id)) = self.project_navigation.find_thread_for_cwd(&cwd) {
            return self
                .activate_project_thread(tui, app_server, thread_id, cwd)
                .await;
        }
        match target {
            ProjectOpenTarget::Tab(placement) => {
                self.start_project_thread(tui, app_server, cwd, placement)
                    .await?;
            }
            ProjectOpenTarget::SplitPane(split_target) => {
                let Some(current_thread_id) = self.active_thread_id else {
                    return Ok(());
                };
                let target_thread_id = self
                    .start_project_thread(tui, app_server, cwd, ProjectTabPlacement::Right)
                    .await?;
                self.bind_split_pane_thread(current_thread_id, target_thread_id, split_target);
            }
        }
        Ok(())
    }

    pub(super) async fn open_new_project_session_at_target(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        cwd: PathBuf,
        target: ProjectOpenTarget,
    ) -> Result<()> {
        match target {
            ProjectOpenTarget::Tab(placement) => {
                self.start_project_thread(tui, app_server, cwd, placement)
                    .await?;
            }
            ProjectOpenTarget::SplitPane(split_target) => {
                let Some(current_thread_id) = self.active_thread_id else {
                    return Ok(());
                };
                let target_thread_id = self
                    .start_project_thread(tui, app_server, cwd, ProjectTabPlacement::Right)
                    .await?;
                self.bind_split_pane_thread(current_thread_id, target_thread_id, split_target);
            }
        }
        Ok(())
    }

    pub(super) async fn resume_project_at_target(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        cwd: PathBuf,
        target: ProjectOpenTarget,
    ) -> Result<()> {
        let mut picker_config = self.rebuild_config_for_cwd(cwd).await?;
        self.apply_runtime_policy_overrides(&mut picker_config);
        let picker_app_server = crate::start_app_server_for_picker(
            &picker_config,
            &self.app_server_target,
            self.state_db.clone(),
            self.environment_manager.clone(),
        )
        .await?;
        if let SessionSelection::Resume(target_session) =
            crate::resume_picker::run_resume_picker_from_existing_session_with_app_server(
                tui,
                &picker_config,
                /*show_all*/ false,
                /*include_non_interactive*/ false,
                picker_app_server,
            )
            .await?
        {
            let resumed = app_server
                .resume_thread(picker_config.clone(), target_session.thread_id)
                .await?;
            let thread_id = resumed.session.thread_id;
            let channel = self.ensure_thread_channel(thread_id);
            channel
                .store
                .lock()
                .await
                .set_session(resumed.session, resumed.turns);
            let current_thread_id = self.active_thread_id;
            let placement = match target {
                ProjectOpenTarget::Tab(placement) => placement,
                ProjectOpenTarget::SplitPane(_) => ProjectTabPlacement::Right,
            };
            self.project_navigation.insert_project_thread(
                thread_id,
                picker_config.cwd.to_path_buf(),
                current_thread_id,
                match placement {
                    ProjectTabPlacement::Left => NewTabPlacement::Left,
                    ProjectTabPlacement::Right => NewTabPlacement::Right,
                },
            );
            if let (Some(current_thread_id), ProjectOpenTarget::SplitPane(split_target)) =
                (current_thread_id, target)
            {
                self.bind_split_pane_thread(current_thread_id, thread_id, split_target);
            }
            self.config = picker_config;
            self.upsert_agent_picker_thread(thread_id, None, None, false);
            self.select_agent_thread(tui, app_server, thread_id).await?;
            self.project_navigation
                .record_recent(self.config.cwd.to_path_buf());
            self.persist_project_navigation_state("resuming a project session");
            self.sync_project_tabs_chrome();
        }
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    pub(super) async fn close_project_tab(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) -> Result<()> {
        let Some(next_thread_id) = self
            .project_navigation
            .adjacent_tab(Some(thread_id), ProjectTabDirection::Next)
        else {
            self.chat_widget
                .add_info_message("Can't close the last project tab.".to_string(), None);
            return Ok(());
        };
        self.project_navigation.remove_thread(thread_id);
        self.thread_event_channels.remove(&thread_id);
        self.agent_navigation.remove(thread_id);
        let _ = app_server.thread_unsubscribe(thread_id).await;
        let cwd = self
            .project_navigation
            .tab_cwd(next_thread_id)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.config.cwd.to_path_buf());
        self.activate_project_thread(tui, app_server, next_thread_id, cwd)
            .await
    }

    pub(super) async fn switch_project_tab(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        direction: ProjectTabDirection,
    ) -> Result<()> {
        let Some(thread_id) = self
            .project_navigation
            .adjacent_tab(self.active_thread_id, direction)
        else {
            return Ok(());
        };
        let cwd = self
            .project_navigation
            .tab_cwd(thread_id)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.config.cwd.to_path_buf());
        self.activate_project_thread(tui, app_server, thread_id, cwd)
            .await
    }

    pub(super) async fn switch_project_workspace(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        direction: ProjectWorkspaceDirection,
    ) -> Result<()> {
        if let Some(thread_id) = self.project_navigation.switch_workspace(direction) {
            let cwd = self
                .project_navigation
                .tab_cwd(thread_id)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.config.cwd.to_path_buf());
            self.activate_project_thread(tui, app_server, thread_id, cwd)
                .await?;
        } else {
            self.sync_project_tabs_chrome();
            self.open_project_navigator();
        }
        Ok(())
    }

    pub(super) async fn focus_split_pane(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        next: bool,
    ) -> Result<()> {
        let Some(split_pane) = self.split_pane.as_mut() else {
            return Ok(());
        };
        let thread_id = if next {
            split_pane.focus_next()
        } else {
            split_pane.focus_previous()
        };
        let Some(thread_id) = thread_id else {
            return Ok(());
        };
        let cwd = self
            .project_navigation
            .tab_cwd(thread_id)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.config.cwd.to_path_buf());
        self.activate_project_thread(tui, app_server, thread_id, cwd)
            .await
    }
}
