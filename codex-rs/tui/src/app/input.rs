//! Keyboard input, external editor, and status-line dispatch for the TUI app.
//!
//! This module owns global key bindings that sit above ChatWidget, including transcript overlay
//! entry, Ctrl-L clear, external editor launch, and agent navigation shortcuts.

use super::*;
use crate::app_event::SplitAxis;
use crate::tui::GamepadAction;
use project_navigation::ProjectTabDirection;
use project_navigation::ProjectWorkspaceDirection;

const SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE: &str =
    "Editing previous prompts is unavailable in side conversations.";

fn project_tab_shortcut_direction(key_event: KeyEvent) -> Option<ProjectTabDirection> {
    if key_event.kind != KeyEventKind::Press || key_event.modifiers != KeyModifiers::ALT {
        return None;
    }
    match key_event.code {
        KeyCode::PageUp => Some(ProjectTabDirection::Previous),
        KeyCode::PageDown => Some(ProjectTabDirection::Next),
        _ => None,
    }
}

fn open_project_navigator_shortcut_matches(key_event: KeyEvent) -> bool {
    key_event.kind == KeyEventKind::Press
        && key_event.modifiers == KeyModifiers::ALT
        && matches!(key_event.code, KeyCode::Char('o') | KeyCode::Char('O'))
}

impl App {
    pub(super) async fn handle_gamepad_action(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        action: GamepadAction,
    ) {
        let active_view_override = self.chat_widget.active_bottom_pane_gamepad_key(action);
        let key = match action {
            GamepadAction::Up => Some(KeyCode::Up),
            GamepadAction::Down => Some(KeyCode::Down),
            GamepadAction::Left => Some(KeyCode::Left),
            GamepadAction::Right => Some(KeyCode::Right),
            GamepadAction::OpenProjectNavigator => {
                self.open_project_navigator();
                None
            }
            GamepadAction::Confirm => active_view_override.or(Some(KeyCode::Enter)),
            GamepadAction::Submit => Some(KeyCode::Enter),
            GamepadAction::Cancel => Some(KeyCode::Esc),
            GamepadAction::Context => active_view_override,
            GamepadAction::Alternate => Some(KeyCode::Char('x')),
            GamepadAction::ProjectTabPrevious => {
                let _ = self
                    .switch_project_tab(tui, app_server, ProjectTabDirection::Previous)
                    .await;
                None
            }
            GamepadAction::ProjectTabNext => {
                let _ = self
                    .switch_project_tab(tui, app_server, ProjectTabDirection::Next)
                    .await;
                None
            }
            GamepadAction::ProjectWorkspacePrevious => {
                let _ = self
                    .switch_project_workspace(tui, app_server, ProjectWorkspaceDirection::Previous)
                    .await;
                None
            }
            GamepadAction::ProjectWorkspaceNext => {
                let _ = self
                    .switch_project_workspace(tui, app_server, ProjectWorkspaceDirection::Next)
                    .await;
                None
            }
            GamepadAction::PreviousPage | GamepadAction::ScrollTranscriptUp => {
                Some(KeyCode::PageUp)
            }
            GamepadAction::NextPage | GamepadAction::ScrollTranscriptDown => {
                Some(KeyCode::PageDown)
            }
            GamepadAction::FocusNext => Some(KeyCode::Tab),
            GamepadAction::ProjectNewTabLeft => {
                self.open_project_chooser(
                    self.config.cwd.to_path_buf(),
                    crate::app_event::ProjectOpenTarget::Tab(
                        crate::app_event::ProjectTabPlacement::Left,
                    ),
                );
                None
            }
            GamepadAction::ProjectNewTabRight => {
                self.open_project_chooser(
                    self.config.cwd.to_path_buf(),
                    crate::app_event::ProjectOpenTarget::Tab(
                        crate::app_event::ProjectTabPlacement::Right,
                    ),
                );
                None
            }
            GamepadAction::PushToTalkStart => {
                self.handle_handy_push_to_talk(/*pressed*/ true);
                None
            }
            GamepadAction::PushToTalkStop => {
                self.handle_handy_push_to_talk(/*pressed*/ false);
                None
            }
            GamepadAction::SplitPaneFocusPrevious => {
                let _ = self.focus_split_pane(tui, app_server, false).await;
                None
            }
            GamepadAction::SplitPaneFocusNext => {
                let _ = self.focus_split_pane(tui, app_server, true).await;
                None
            }
            GamepadAction::SplitPaneCreateHorizontal => {
                self.open_split_pane_project_chooser(SplitAxis::Horizontal);
                None
            }
            GamepadAction::SplitPaneCreateVertical => {
                self.open_split_pane_project_chooser(SplitAxis::Vertical);
                None
            }
        };
        if let Some(code) = key {
            self.handle_key_event(tui, app_server, KeyEvent::from(code))
                .await;
        }
    }

    fn handle_handy_push_to_talk(&mut self, pressed: bool) {
        #[cfg(feature = "voice-input")]
        {
            const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);
            let now = Instant::now();

            if pressed {
                let is_double_tap = self
                    .handy_gamepad
                    .last_release_at
                    .is_some_and(|last| now.saturating_duration_since(last) <= DOUBLE_TAP_WINDOW);

                if is_double_tap {
                    self.handy_gamepad.last_release_at = None;
                    self.toggle_handy_continuous_mode();
                    return;
                }

                if !self.handy_gamepad.continuous_mode {
                    self.start_handy_recording();
                }
                return;
            }

            self.handy_gamepad.last_release_at = Some(now);
            if !self.handy_gamepad.continuous_mode {
                self.stop_handy_recording();
            }
        }

        #[cfg(not(feature = "voice-input"))]
        {
            let _ = pressed;
        }
    }

    #[cfg(feature = "voice-input")]
    fn toggle_handy_continuous_mode(&mut self) {
        self.handy_gamepad.continuous_mode = !self.handy_gamepad.continuous_mode;
        if self.handy_gamepad.continuous_mode {
            Self::play_handy_chime();
            self.start_handy_recording();
        } else {
            self.stop_handy_recording();
        }
    }

    #[cfg(feature = "voice-input")]
    fn start_handy_recording(&mut self) {
        if self.handy_gamepad.voice.is_some() {
            return;
        }

        match crate::voice::VoiceCapture::start() {
            Ok(voice) => {
                Self::play_handy_chime();
                let placeholder_id = self.chat_widget.insert_recording_meter_placeholder("⠤⠤⠤⠤");
                self.spawn_handy_recording_meter(
                    placeholder_id.clone(),
                    voice.last_peak_arc(),
                    voice.stopped_flag(),
                );
                self.handy_gamepad.voice = Some(voice);
                self.handy_gamepad.placeholder_id = Some(placeholder_id);
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to start Handy voice capture: {err}"));
            }
        }
    }

    #[cfg(feature = "voice-input")]
    fn stop_handy_recording(&mut self) {
        let Some(voice) = self.handy_gamepad.voice.take() else {
            return;
        };

        let placeholder_id = self
            .handy_gamepad
            .placeholder_id
            .clone()
            .unwrap_or_else(|| self.chat_widget.insert_recording_meter_placeholder("⠤⠤⠤⠤"));

        match voice.stop() {
            Ok(audio) => {
                let total_samples = audio.data.len() as f32;
                let samples_per_second = (audio.sample_rate as f32) * (audio.channels as f32);
                let duration_seconds = if samples_per_second > 0.0 {
                    total_samples / samples_per_second
                } else {
                    0.0
                };
                if duration_seconds < 0.25 {
                    self.chat_widget
                        .remove_recording_meter_placeholder(&placeholder_id);
                    self.handy_gamepad.placeholder_id = None;
                    return;
                }

                let prompt_source = self.chat_widget.composer_text_with_pending();
                crate::voice::transcribe_async(
                    placeholder_id,
                    audio,
                    Some(prompt_source),
                    self.app_event_tx.clone(),
                );
            }
            Err(err) => {
                self.chat_widget
                    .remove_recording_meter_placeholder(&placeholder_id);
                self.handy_gamepad.placeholder_id = None;
                self.chat_widget
                    .add_error_message(format!("Failed to stop Handy voice capture: {err}"));
            }
        }
    }

    #[cfg(feature = "voice-input")]
    fn spawn_handy_recording_meter(
        &self,
        id: String,
        last_peak: Arc<AtomicU16>,
        stop: Arc<AtomicBool>,
    ) {
        let tx = self.app_event_tx.clone();
        let task = move || {
            let mut meter = crate::voice::RecordingMeterState::new();
            while !stop.load(Ordering::Relaxed) {
                tx.send(AppEvent::UpdateRecordingMeter {
                    id: id.clone(),
                    text: meter.next_text(last_peak.load(Ordering::Relaxed)),
                });
                thread::sleep(Duration::from_millis(100));
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(task);
        } else {
            thread::spawn(task);
        }
    }

    #[cfg(feature = "voice-input")]
    fn play_handy_chime() {
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(b"\x07");
        let _ = stderr.flush();
    }

    pub(super) async fn launch_external_editor(&mut self, tui: &mut tui::Tui) {
        let editor_cmd = match external_editor::resolve_editor_command() {
            Ok(cmd) => cmd,
            Err(external_editor::EditorError::MissingEditor) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(
                    "Cannot open external editor: set $VISUAL or $EDITOR before starting Codex."
                        .to_string(),
                ));
                self.reset_external_editor_state(tui);
                return;
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
                self.reset_external_editor_state(tui);
                return;
            }
        };

        let seed = self.chat_widget.composer_text_with_pending();
        let editor_result = tui
            .with_restored(tui::RestoreMode::KeepRaw, || async {
                external_editor::run_editor(&seed, &editor_cmd).await
            })
            .await;
        self.reset_external_editor_state(tui);

        match editor_result {
            Ok(new_text) => {
                // Trim trailing whitespace
                let cleaned = new_text.trim_end().to_string();
                self.chat_widget.apply_external_edit(cleaned);
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
            }
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn request_external_editor_launch(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Requested);
        self.chat_widget.set_footer_hint_override(Some(vec![(
            EXTERNAL_EDITOR_HINT.to_string(),
            String::new(),
        )]));
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn reset_external_editor_state(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Closed);
        self.chat_widget.set_footer_hint_override(/*items*/ None);
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn apply_raw_output_mode(
        &mut self,
        tui: &mut tui::Tui,
        enabled: bool,
        notify: bool,
    ) {
        if notify {
            self.chat_widget.set_raw_output_mode_and_notify(enabled);
        } else {
            self.chat_widget.set_raw_output_mode(enabled);
        }
        if let Err(err) = self.reflow_transcript_now(tui) {
            tracing::warn!(error = %err, "failed to reflow transcript after raw output mode toggle");
            self.chat_widget
                .add_error_message(format!("Failed to redraw transcript: {err}"));
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn handle_key_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        key_event: KeyEvent,
    ) {
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            && let Some(direction) = project_tab_shortcut_direction(key_event)
        {
            let _ = self.switch_project_tab(tui, app_server, direction).await;
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            && open_project_navigator_shortcut_matches(key_event)
        {
            self.open_project_navigator();
            return;
        }

        // Some terminals, especially on macOS, encode Option+Left/Right as Option+b/f unless
        // enhanced keyboard reporting is available. We only treat those word-motion fallbacks as
        // agent-switch shortcuts when the composer is empty so we never steal the expected
        // editing behavior for moving across words inside a draft.
        let allow_agent_word_motion_fallback = !self.enhanced_keys_supported
            && self.chat_widget.composer_text_with_pending().is_empty();
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Alt+Left/Right are also natural word-motion keys in the composer. Keep agent
            // fast-switch available only once the draft is empty so editing behavior wins whenever
            // there is text on screen.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && previous_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Previous)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Mirror the previous-agent rule above: empty drafts may use these keys for thread
            // switching, but non-empty drafts keep them for expected word-wise cursor motion.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && next_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Next)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if side_return_shortcut_matches(key_event)
            && self.maybe_return_from_side(tui, app_server).await
        {
            return;
        }

        let app_keymap_shortcuts_available = self.app_keymap_shortcuts_available();

        if app_keymap_shortcuts_available && self.keymap.app.toggle_vim_mode.is_pressed(key_event) {
            self.chat_widget.toggle_vim_mode_and_notify();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.toggle_fast_mode.is_pressed(key_event)
            && self.chat_widget.can_toggle_fast_mode_from_keybinding()
        {
            self.chat_widget.toggle_fast_mode_from_ui();
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.toggle_raw_output.is_pressed(key_event)
        {
            let enabled = !self.chat_widget.raw_output_mode();
            self.apply_raw_output_mode(tui, enabled, /*notify*/ false);
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.open_transcript.is_pressed(key_event) {
            // Enter alternate screen and set viewport to full size.
            let _ = tui.enter_alt_screen();
            self.overlay = Some(Overlay::new_transcript(
                self.transcript_cells.clone(),
                self.keymap.pager.clone(),
            ));
            tui.frame_requester().schedule_frame();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.open_external_editor.is_pressed(key_event)
        {
            // Only launch the external editor if there is no overlay and the bottom pane is not in use.
            // Note that it can be launched while a task is running to enable editing while the previous turn is ongoing.
            if self.overlay.is_none()
                && self.chat_widget.can_launch_external_editor()
                && self.chat_widget.external_editor_state() == ExternalEditorState::Closed
            {
                self.request_external_editor_launch(tui);
            }
            return;
        }

        if matches!(key_event.code, KeyCode::Esc)
            && matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            // Esc primes/advances backtracking only in normal (not working) mode
            // with the composer focused and empty. In any other state, forward
            // Esc so the active UI (e.g. status indicator, modals, popups)
            // handles it.
            if self.should_handle_backtrack_esc(key_event) {
                self.handle_backtrack_esc_key(tui);
            } else if self.should_reject_side_backtrack_esc(key_event) {
                self.reject_side_backtrack_esc();
            } else {
                self.chat_widget.handle_key_event(key_event);
            }
            return;
        }

        match key_event {
            _ if app_keymap_shortcuts_available
                && self.keymap.app.clear_terminal.is_pressed(key_event) =>
            {
                if !self.chat_widget.can_run_ctrl_l_clear_now() {
                    return;
                }
                if let Err(err) = self.clear_terminal_ui(tui, /*redraw_header*/ false) {
                    tracing::warn!(error = %err, "failed to clear terminal UI");
                    self.chat_widget
                        .add_error_message(format!("Failed to clear terminal UI: {err}"));
                } else {
                    self.reset_app_ui_state_after_clear();
                    self.queue_clear_ui_header(tui);
                    tui.frame_requester().schedule_frame();
                }
            }
            // Enter confirms backtrack when primed + count > 0. Otherwise pass to widget.
            KeyEvent {
                code: KeyCode::Enter,
                kind: KeyEventKind::Press,
                ..
            } if self.backtrack.primed
                && self.backtrack.nth_user_message != usize::MAX
                && self.chat_widget.composer_is_empty() =>
            {
                if let Some(selection) = self.confirm_backtrack_from_main() {
                    self.apply_backtrack_selection(tui, selection);
                }
            }
            KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // Any non-Esc key press should cancel a primed backtrack.
                // This avoids stale "Esc-primed" state after the user starts typing
                // (even if they later backspace to empty).
                if key_event.code != KeyCode::Esc && self.backtrack.primed {
                    self.reset_backtrack_state();
                }
                self.chat_widget.handle_key_event(key_event);
            }
            _ => {
                self.chat_widget.handle_key_event(key_event);
            }
        };
    }

    pub(super) fn should_handle_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        !self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn should_reject_side_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn reject_side_backtrack_esc(&mut self) {
        self.reset_backtrack_state();
        self.chat_widget
            .add_error_message(SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE.to_string());
    }

    fn app_keymap_shortcuts_available(&self) -> bool {
        self.overlay.is_none() && self.chat_widget.no_modal_or_popup_active()
    }

    pub(super) fn refresh_status_line(&mut self) {
        self.chat_widget.refresh_status_line();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_test_app;

    #[tokio::test]
    async fn app_keymap_shortcuts_are_disabled_while_keymap_view_is_active() {
        let mut app = make_test_app().await;
        assert!(app.app_keymap_shortcuts_available());

        let keymap = app.keymap.clone();
        app.chat_widget.open_keymap_debug(&keymap);

        assert!(!app.app_keymap_shortcuts_available());
    }
}
