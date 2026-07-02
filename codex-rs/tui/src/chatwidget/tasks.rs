use super::*;
use crate::task_picker;

impl ChatWidget {
    pub(crate) fn open_task_picker(&mut self) {
        self.task_picker_request_id = self.task_picker_request_id.wrapping_add(1);
        let request_id = self.task_picker_request_id;
        self.task_picker_action_cache.clear();

        self.bottom_pane
            .show_view(Box::new(task_picker::TaskPickerView::loading(
                self.app_event_tx.clone(),
                "Loading Palace Plane tasks...".to_string(),
            )));

        let app_event_tx = self.app_event_tx.clone();
        let cwd = self.config.cwd.to_path_buf();
        tokio::spawn(async move {
            let payload =
                task_picker::load_task_picker_payload(cwd, task_picker::TaskPickerLoadTarget::Auto)
                    .await;
            app_event_tx.send(AppEvent::TaskPickerPayloadLoaded {
                request_id,
                payload,
            });
        });
    }

    pub(crate) fn load_task_picker_project(
        &mut self,
        workspace: String,
        project_slug: String,
        project_name: String,
    ) {
        self.task_picker_request_id = self.task_picker_request_id.wrapping_add(1);
        let request_id = self.task_picker_request_id;
        self.task_picker_action_cache.clear();

        self.bottom_pane.replace_view_if_active(
            task_picker::TASKS_SELECTION_VIEW_ID,
            Box::new(task_picker::TaskPickerView::loading(
                self.app_event_tx.clone(),
                format!("Loading {workspace}/{project_slug}..."),
            )),
        );

        let app_event_tx = self.app_event_tx.clone();
        let cwd = self.config.cwd.to_path_buf();
        tokio::spawn(async move {
            let payload = task_picker::load_task_picker_payload(
                cwd,
                task_picker::TaskPickerLoadTarget::Project {
                    workspace,
                    project_slug,
                    project_name: Some(project_name),
                },
            )
            .await;
            app_event_tx.send(AppEvent::TaskPickerPayloadLoaded {
                request_id,
                payload,
            });
        });
    }

    pub(crate) fn apply_task_picker_payload(
        &mut self,
        request_id: u64,
        payload: task_picker::TaskPickerPayload,
    ) {
        if request_id != self.task_picker_request_id {
            return;
        }

        self.task_picker_action_cache = match &payload {
            task_picker::TaskPickerPayload::TaskList { tasks, .. } => tasks
                .iter()
                .filter(|task| task.plane_issue_id.is_some())
                .cloned()
                .collect(),
            task_picker::TaskPickerPayload::ProjectList { .. }
            | task_picker::TaskPickerPayload::Error { .. } => Vec::new(),
        };

        self.bottom_pane.replace_view_if_active(
            task_picker::TASKS_SELECTION_VIEW_ID,
            Box::new(task_picker::TaskPickerView::from_payload(
                self.app_event_tx.clone(),
                payload,
            )),
        );
    }

    pub(crate) fn open_task_action_menu(&mut self, task_ids: Vec<String>) {
        if let Some(params) = task_picker::task_action_menu_params(
            &self.task_picker_action_cache,
            &task_ids,
            self.app_event_tx.clone(),
        ) {
            self.bottom_pane.show_selection_view(params);
        } else {
            self.add_info_message(
                "No task actions are available for that selection.".to_string(),
                None,
            );
        }
    }
}
