use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use palace_plane::GlobalConfig;
use palace_plane::PlaneClient;
use palace_plane::ProjectConfig;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use serde::Deserialize;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::ScrollState;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::render::renderable::Renderable;

pub(crate) const TASKS_SELECTION_VIEW_ID: &str = "palace-task-picker";
const DEFAULT_WORKSPACE: &str = "riffcc";

#[derive(Clone, Debug)]
pub(crate) enum TaskSource {
    Plane,
}

impl TaskSource {
    fn label(&self) -> &'static str {
        match self {
            Self::Plane => "Plane",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskKind {
    RealIssue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskState {
    Ready,
    InProgress,
    Done,
    Blocked,
}

impl TaskState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::InProgress => "In Progress",
            Self::Done => "Done",
            Self::Blocked => "Blocked",
        }
    }

    fn can_continue(self) -> bool {
        matches!(self, Self::InProgress | Self::Blocked)
    }

    fn is_done(self) -> bool {
        matches!(self, Self::Done)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TaskItem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) details: Vec<String>,
    pub(crate) prompt: String,
    pub(crate) source: TaskSource,
    pub(crate) state: TaskState,
    pub(crate) branch: Option<String>,
    pub(crate) ci_status: Option<String>,
    pub(crate) plane_issue_id: Option<String>,
    kind: TaskKind,
}

impl TaskItem {
    fn is_real_issue(&self) -> bool {
        self.kind == TaskKind::RealIssue
    }
}

#[derive(Clone, Debug)]
struct ProjectPickerItem {
    workspace: String,
    project_slug: String,
    name: String,
    description: Option<String>,
}

#[derive(Clone, Debug)]
enum TaskPickerEntry {
    Project(ProjectPickerItem),
    Task(TaskItem),
}

impl TaskPickerEntry {
    fn title(&self) -> &str {
        match self {
            Self::Project(project) => &project.name,
            Self::Task(task) => &task.title,
        }
    }

    fn summary(&self) -> &str {
        match self {
            Self::Project(project) => project.description.as_deref().unwrap_or(""),
            Self::Task(task) => &task.summary,
        }
    }

    fn source_label(&self) -> &'static str {
        match self {
            Self::Project(_) => "Plane Project",
            Self::Task(task) => task.source.label(),
        }
    }

    fn project_slug(&self) -> Option<&str> {
        match self {
            Self::Project(project) => Some(&project.project_slug),
            Self::Task(_) => None,
        }
    }

    fn task(&self) -> Option<&TaskItem> {
        match self {
            Self::Project(_) => None,
            Self::Task(task) => Some(task),
        }
    }

    fn project(&self) -> Option<&ProjectPickerItem> {
        match self {
            Self::Project(project) => Some(project),
            Self::Task(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
enum TaskPickerMode {
    Loading {
        message: String,
    },
    ProjectList {
        workspace: String,
    },
    TaskList {
        workspace: String,
        project_slug: String,
        project_name: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum TaskPickerLoadTarget {
    Auto,
    Project {
        workspace: String,
        project_slug: String,
        project_name: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum TaskPickerPayload {
    ProjectList {
        workspace: String,
        projects: Vec<TaskPickerProject>,
    },
    TaskList {
        workspace: String,
        project_slug: String,
        project_name: String,
        tasks: Vec<TaskItem>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct TaskPickerProject {
    pub(crate) workspace: String,
    pub(crate) project_slug: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
}

pub(crate) struct TaskPickerView {
    app_event_tx: AppEventSender,
    mode: TaskPickerMode,
    entries: Vec<TaskPickerEntry>,
    state: ScrollState,
    filtered_indices: Vec<usize>,
    search_query: String,
    selected_ids: HashSet<String>,
    complete: bool,
}

impl TaskPickerView {
    pub(crate) fn loading(app_event_tx: AppEventSender, message: String) -> Self {
        Self {
            app_event_tx,
            mode: TaskPickerMode::Loading { message },
            entries: Vec::new(),
            state: ScrollState::new(),
            filtered_indices: Vec::new(),
            search_query: String::new(),
            selected_ids: HashSet::new(),
            complete: false,
        }
    }

    pub(crate) fn from_payload(app_event_tx: AppEventSender, payload: TaskPickerPayload) -> Self {
        let (mode, entries) = match payload {
            TaskPickerPayload::ProjectList {
                workspace,
                projects,
            } => {
                let entries = projects
                    .into_iter()
                    .map(|project| {
                        TaskPickerEntry::Project(ProjectPickerItem {
                            workspace: project.workspace,
                            project_slug: project.project_slug,
                            name: project.name,
                            description: project.description,
                        })
                    })
                    .collect::<Vec<_>>();
                (TaskPickerMode::ProjectList { workspace }, entries)
            }
            TaskPickerPayload::TaskList {
                workspace,
                project_slug,
                project_name,
                tasks,
            } => {
                let entries = tasks
                    .into_iter()
                    .map(TaskPickerEntry::Task)
                    .collect::<Vec<_>>();
                (
                    TaskPickerMode::TaskList {
                        workspace,
                        project_slug,
                        project_name,
                    },
                    entries,
                )
            }
            TaskPickerPayload::Error { message } => (TaskPickerMode::Error { message }, Vec::new()),
        };

        let mut state = ScrollState::new();
        state.selected_idx = (!entries.is_empty()).then_some(0);
        let filtered_indices = (0..entries.len()).collect();

        Self {
            app_event_tx,
            mode,
            entries,
            state,
            filtered_indices,
            search_query: String::new(),
            selected_ids: HashSet::new(),
            complete: false,
        }
    }

    fn visible_len(&self) -> usize {
        self.filtered_indices.len()
    }

    fn visible_rows(&self) -> usize {
        MAX_POPUP_ROWS.min(self.visible_len().max(1))
    }

    fn apply_filter(&mut self) {
        let selected_actual = self
            .state
            .selected_idx
            .and_then(|idx| self.filtered_indices.get(idx).copied());
        let filter = self.search_query.trim().to_ascii_lowercase();

        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                if filter.is_empty() {
                    return true;
                }

                entry.title().to_ascii_lowercase().contains(&filter)
                    || entry.summary().to_ascii_lowercase().contains(&filter)
                    || entry.source_label().to_ascii_lowercase().contains(&filter)
                    || entry
                        .project_slug()
                        .is_some_and(|slug| slug.to_ascii_lowercase().contains(&filter))
                    || entry
                        .task()
                        .and_then(|task| task.plane_issue_id.as_deref())
                        .is_some_and(|id| id.to_ascii_lowercase().contains(&filter))
            })
            .map(|(idx, _)| idx)
            .collect();

        let len = self.visible_len();
        self.state.selected_idx = selected_actual
            .and_then(|actual| self.filtered_indices.iter().position(|idx| *idx == actual))
            .or_else(|| (len > 0).then_some(0));
        self.state.clamp_selection(len);
        self.state.ensure_visible(len, self.visible_rows());
    }

    fn move_up(&mut self) {
        let len = self.visible_len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, self.visible_rows());
    }

    fn move_down(&mut self) {
        let len = self.visible_len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, self.visible_rows());
    }

    fn focused_entry(&self) -> Option<&TaskPickerEntry> {
        self.state
            .selected_idx
            .and_then(|idx| self.filtered_indices.get(idx))
            .and_then(|idx| self.entries.get(*idx))
    }

    fn focused_task(&self) -> Option<&TaskItem> {
        self.focused_entry().and_then(TaskPickerEntry::task)
    }

    fn focused_project(&self) -> Option<&ProjectPickerItem> {
        self.focused_entry().and_then(TaskPickerEntry::project)
    }

    fn selected_tasks(&self) -> Vec<&TaskItem> {
        self.entries
            .iter()
            .filter_map(TaskPickerEntry::task)
            .filter(|task| task.is_real_issue() && self.selected_ids.contains(&task.id))
            .collect()
    }

    fn active_tasks(&self) -> Vec<&TaskItem> {
        let selected = self.selected_tasks();
        if selected.is_empty() {
            self.focused_task()
                .filter(|task| task.is_real_issue())
                .into_iter()
                .collect()
        } else {
            selected
        }
    }

    fn toggle_selected(&mut self) {
        let Some(task) = self.focused_task() else {
            return;
        };
        if !task.is_real_issue() {
            return;
        }

        let task_id = task.id.clone();
        if !self.selected_ids.insert(task_id.clone()) {
            self.selected_ids.remove(&task_id);
        }
    }

    fn open_context_menu(&self) {
        let ids = self
            .active_tasks()
            .into_iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        self.app_event_tx
            .send(AppEvent::OpenTaskActionMenu { task_ids: ids });
    }

    fn submit_default(&self) {
        if let Some(project) = self.focused_project() {
            self.app_event_tx.send(AppEvent::LoadTaskPickerProject {
                workspace: project.workspace.clone(),
                project_slug: project.project_slug.clone(),
                project_name: project.name.clone(),
            });
            return;
        }

        let selected = self.selected_tasks();
        if !selected.is_empty() {
            let prompt = if selected.iter().any(|task| task.state.can_continue()) {
                continue_implementation_prompt(&selected)
            } else {
                plan_and_implement_prompt(&selected)
            };
            self.app_event_tx
                .send(AppEvent::SetComposerText { text: prompt });
            return;
        }

        let Some(task) = self.focused_task() else {
            return;
        };

        let prompt = if task.is_real_issue() {
            if task.state.can_continue() {
                continue_implementation_prompt(&[task])
            } else {
                plan_and_implement_prompt(&[task])
            }
        } else {
            task.prompt.clone()
        };
        self.app_event_tx
            .send(AppEvent::SetComposerText { text: prompt });
    }

    fn rows(&self) -> Vec<Line<'static>> {
        let start = self.state.scroll_top;
        let end = (start + self.visible_rows()).min(self.visible_len());
        self.filtered_indices[start..end]
            .iter()
            .enumerate()
            .filter_map(|(offset, idx)| self.entries.get(*idx).map(|entry| (offset, entry)))
            .map(|(offset, entry)| {
                let actual_visible_idx = start + offset;
                let focused = self.state.selected_idx == Some(actual_visible_idx);

                match entry {
                    TaskPickerEntry::Project(project) => Line::from(vec![
                        Span::styled(
                            if focused { "›" } else { " " },
                            if focused {
                                Style::default().add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                        Span::raw("   "),
                        Span::styled(
                            project.name.clone(),
                            if focused {
                                Style::default().add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                        Span::raw(" "),
                        Span::raw(format!("{} · {}", project.workspace, project.project_slug))
                            .dim(),
                    ]),
                    TaskPickerEntry::Task(task) => {
                        let selectable = task.is_real_issue();
                        let selected = selectable && self.selected_ids.contains(&task.id);
                        let checkbox = if selectable {
                            if selected { "[x]" } else { "[ ]" }
                        } else {
                            " · "
                        };

                        let mut line = Line::from(vec![
                            Span::styled(
                                if focused { "›" } else { " " },
                                if focused {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            Span::raw(" "),
                            Span::raw(checkbox),
                            Span::raw(" "),
                            Span::styled(
                                task.title.clone(),
                                if focused {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            Span::raw(" "),
                            Span::raw(format!("{} · {}", task.source.label(), task.state.label()))
                                .dim(),
                        ]);
                        if let Some(issue) = &task.plane_issue_id {
                            line.spans.push(Span::raw(" "));
                            line.spans.push(Span::raw(issue.clone()).dim());
                        }
                        line
                    }
                }
            })
            .collect()
    }

    fn details_lines(&self) -> Vec<Line<'static>> {
        if let TaskPickerMode::Loading { message } = &self.mode {
            return vec![
                Line::from("Loading tasks".bold()),
                Line::from(""),
                Line::from(message.clone()),
            ];
        }

        if let TaskPickerMode::Error { message } = &self.mode {
            return vec![
                Line::from("Task picker load failed".bold()),
                Line::from(""),
                Line::from(message.clone()),
                Line::from(""),
                Line::from(
                    "Press Esc to close and run /tasks again after fixing config/API access.",
                ),
            ];
        }

        if let Some(project) = self.focused_project() {
            let mut lines = vec![
                Line::from(project.name.clone().bold()),
                Line::from(
                    format!(
                        "Workspace: {} · Project: {}",
                        project.workspace, project.project_slug
                    )
                    .dim(),
                ),
                Line::from(""),
                Line::from(project.description.clone().unwrap_or_else(|| {
                    "Browse this project to load active Plane issues and summary rows.".to_string()
                })),
                Line::from(""),
                Line::from("Press Enter to open this project."),
            ];
            if self.selected_ids.is_empty() {
                lines.push(Line::from(
                    "Space/X actions apply only to real issue rows.".dim(),
                ));
            }
            return lines;
        }

        let selected = self.selected_tasks();
        if selected.len() > 1 {
            let done = selected.iter().filter(|task| task.state.is_done()).count();
            let in_progress = selected
                .iter()
                .filter(|task| task.state == TaskState::InProgress)
                .count();
            let mut lines = vec![
                Line::from(format!("{} tasks selected", selected.len()).bold()),
                Line::from(
                    format!(
                        "{done} done · {in_progress} in progress · {} actionable",
                        selected.len()
                    )
                    .dim(),
                ),
                Line::from(""),
                Line::from("Batch actions".bold()),
                Line::from("X opens the batch action menu for this selection."),
                Line::from("Y/Enter stages the default action into the composer."),
                Line::from(""),
            ];
            for task in selected {
                lines.push(Line::from(format!(
                    "• {} ({})",
                    task.title,
                    task.state.label()
                )));
            }
            return lines;
        }

        let Some(task) = self.focused_task() else {
            return vec![Line::from("No tasks available.")];
        };

        let mut lines = vec![
            Line::from(task.title.clone().bold()),
            Line::from(format!("{} · {}", task.source.label(), task.state.label()).dim()),
        ];
        if let Some(issue) = &task.plane_issue_id {
            lines.push(Line::from(format!("Plane: {issue}").dim()));
        }
        if let Some(branch) = &task.branch {
            lines.push(Line::from(format!("Branch: {branch}").dim()));
        }
        if let Some(ci) = &task.ci_status {
            lines.push(Line::from(format!("CI/CD: {ci}").dim()));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(task.summary.clone()));
        lines.push(Line::from(""));
        lines.push(Line::from("Default staged action".bold()));
        if task.is_real_issue() {
            lines.push(Line::from(if task.state.can_continue() {
                "Continue implementation"
            } else {
                "Plan and implement"
            }));
            lines.push(Line::from(""));
            lines.push(Line::from("Notes".bold()));
            for detail in &task.details {
                lines.push(Line::from(format!("• {detail}")));
            }
        } else {
            lines.push(Line::from("Stage the synthetic summary prompt."));
            lines.push(Line::from(""));
            lines.push(Line::from("Notes".bold()));
            for detail in &task.details {
                lines.push(Line::from(format!("• {detail}")));
            }
        }
        lines
    }

    fn header_lines(&self) -> Vec<Line<'static>> {
        let selected_count = self.selected_ids.len();
        let filter = if self.search_query.is_empty() {
            "none".to_string()
        } else {
            self.search_query.clone()
        };

        match &self.mode {
            TaskPickerMode::Loading { .. } => vec![
                Line::from("Palace Task Picker".bold()),
                Line::from("Opening live tasks...".dim()),
                Line::from(format!("filter: {filter}").dim()),
            ],
            TaskPickerMode::ProjectList { workspace } => vec![
                Line::from("Palace Task Picker".bold()),
                Line::from(format!("Browse Plane projects in workspace {workspace}.").dim()),
                Line::from(
                    format!(
                        "{} projects · {selected_count} selected · filter: {filter}",
                        self.entries.len()
                    )
                    .dim(),
                ),
            ],
            TaskPickerMode::TaskList {
                workspace,
                project_slug,
                project_name,
            } => vec![
                Line::from("Palace Task Picker".bold()),
                Line::from(
                    format!(
                        "{project_name} ({workspace}/{project_slug}) · Space select · X menu · Enter stage"
                    )
                    .dim(),
                ),
                Line::from(
                    format!(
                        "{} rows · {selected_count} selected · filter: {filter}",
                        self.entries.len()
                    )
                    .dim(),
                ),
            ],
            TaskPickerMode::Error { .. } => vec![
                Line::from("Palace Task Picker".bold()),
                Line::from("Failed to load live task data.".dim()),
                Line::from(format!("filter: {filter}").dim()),
            ],
        }
    }
}

impl BottomPaneView for TaskPickerView {
    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(TASKS_SELECTION_VIEW_ID)
    }

    fn map_gamepad_action(&self, action: crate::tui::GamepadAction) -> Option<KeyCode> {
        match action {
            crate::tui::GamepadAction::Confirm => Some(KeyCode::Char(' ')),
            crate::tui::GamepadAction::Submit => Some(KeyCode::Enter),
            crate::tui::GamepadAction::Context => Some(KeyCode::Char('x')),
            _ => None,
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.move_up(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                self.search_query.pop();
                self.apply_filter();
            }
            KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.toggle_selected();
            }
            KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.open_context_menu();
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                self.submit_default();
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.complete = true;
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT)
                && !matches!(ch, ' ' | 'x' | 'y') =>
            {
                self.search_query.push(ch);
                self.apply_filter();
            }
            _ => {}
        }
    }
}

impl Renderable for TaskPickerView {
    fn desired_height(&self, _width: u16) -> u16 {
        20
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(area);
        let [list_area, details_area] =
            Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
                .areas(body_area);

        Paragraph::new(self.header_lines())
            .block(Block::default().borders(Borders::ALL).title("Tasks"))
            .render(header_area, buf);

        let rows = if self.rows().is_empty() {
            vec![Line::from("No matching rows.")]
        } else {
            self.rows()
        };
        Paragraph::new(rows)
            .block(Block::default().borders(Borders::ALL).title("Queue"))
            .render(list_area, buf);

        Paragraph::new(self.details_lines())
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .render(details_area, buf);

        standard_popup_hint_line().render_ref(footer_area, buf);
    }
}

pub(crate) fn task_action_menu_params(
    task_cache: &[TaskItem],
    task_ids: &[String],
    app_event_tx: AppEventSender,
) -> Option<SelectionViewParams> {
    let tasks = task_cache
        .iter()
        .filter(|task| task.is_real_issue() && task_ids.iter().any(|id| id == &task.id))
        .cloned()
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return None;
    }

    let done_ids = tasks
        .iter()
        .filter(|task| task.state.is_done())
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let has_continue = tasks.iter().any(|task| task.state.can_continue());
    let title = if tasks.len() == 1 {
        format!("Actions for {}", tasks[0].title)
    } else {
        format!("Actions for {} selected tasks", tasks.len())
    };

    let mut items = Vec::new();
    items.push(action_item(
        "Check status",
        Some("Stage a status pass for the selected tasks.".to_string()),
        check_status_prompt(&tasks),
        app_event_tx.clone(),
    ));
    items.push(action_item(
        "Run tests",
        Some("Stage a test run scoped to the selected tasks.".to_string()),
        run_tests_prompt(&tasks),
        app_event_tx.clone(),
    ));
    items.push(action_item(
        "Plan and implement",
        Some("Stage a plan/execution prompt for the selected tasks.".to_string()),
        plan_and_implement_prompt(&tasks.iter().collect::<Vec<_>>()),
        app_event_tx.clone(),
    ));
    if has_continue {
        items.push(action_item(
            "Continue implementation",
            Some("Stage a continuation prompt for the active tasks.".to_string()),
            continue_implementation_prompt(&tasks.iter().collect::<Vec<_>>()),
            app_event_tx.clone(),
        ));
    }
    items.push(action_item(
        "Configure task",
        Some("Stage a task-configuration prompt for the selected scope.".to_string()),
        configure_task_prompt(&tasks),
        app_event_tx.clone(),
    ));
    if !done_ids.is_empty() {
        let description = if done_ids.len() == tasks.len() {
            "Stage verification for the selected completed tasks.".to_string()
        } else {
            format!(
                "Stage verification for {} done task(s); incomplete tasks will be skipped.",
                done_ids.len()
            )
        };
        let done_tasks = tasks
            .iter()
            .filter(|task| task.state.is_done())
            .collect::<Vec<_>>();
        items.push(action_item(
            "Verify implementation",
            Some(description),
            verify_implementation_prompt(&done_tasks),
            app_event_tx,
        ));
    }

    Some(SelectionViewParams {
        title: Some(title),
        subtitle: Some("Choose the next staged action".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    })
}

fn action_item(
    name: &str,
    description: Option<String>,
    prompt: String,
    app_event_tx: AppEventSender,
) -> SelectionItem {
    let mut item = SelectionItem {
        name: name.to_string(),
        description,
        dismiss_on_select: true,
        ..Default::default()
    };
    item.actions = vec![Box::new(move |_| {
        app_event_tx.send(AppEvent::SetComposerText {
            text: prompt.clone(),
        });
    })];
    item
}

fn format_task_list(tasks: &[&TaskItem]) -> String {
    tasks
        .iter()
        .map(|task| match &task.plane_issue_id {
            Some(issue) => format!("{} ({issue})", task.title),
            None => task.title.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_status_prompt(tasks: &[TaskItem]) -> String {
    let refs = tasks.iter().collect::<Vec<_>>();
    format!(
        "Check the current status of these tasks: {}. Summarize progress, blockers, CI/CD state, and what changed most recently.",
        format_task_list(&refs)
    )
}

fn run_tests_prompt(tasks: &[TaskItem]) -> String {
    let refs = tasks.iter().collect::<Vec<_>>();
    format!(
        "Work from these tasks: {}. Figure out the most relevant tests or validation steps, run or plan them, and summarize failures or missing coverage before any deeper implementation.",
        format_task_list(&refs)
    )
}

fn plan_and_implement_prompt(tasks: &[&TaskItem]) -> String {
    format!(
        "Plan and implement the work for these tasks: {}. Start with the highest-leverage slice, keep the tasks explicitly mapped to the work, and report what is done vs still open.",
        format_task_list(tasks)
    )
}

fn continue_implementation_prompt(tasks: &[&TaskItem]) -> String {
    format!(
        "Continue implementation for these active tasks: {}. Pick up from the current state, preserve existing progress, and focus on unblocking the next deliverable step.",
        format_task_list(tasks)
    )
}

fn configure_task_prompt(tasks: &[TaskItem]) -> String {
    let refs = tasks.iter().collect::<Vec<_>>();
    format!(
        "Configure the execution strategy for these tasks: {}. Decide the best order, relevant tools, dependencies, and any constraints or approvals that should be attached to each task.",
        format_task_list(&refs)
    )
}

fn verify_implementation_prompt(tasks: &[&TaskItem]) -> String {
    format!(
        "Verify implementation quality for these done tasks: {}. Check the code against the intended outcome, identify regressions or missing tests, and confirm whether each task is actually complete.",
        format_task_list(tasks)
    )
}

pub(crate) async fn load_task_picker_payload(
    cwd: PathBuf,
    target: TaskPickerLoadTarget,
) -> TaskPickerPayload {
    match load_task_picker_payload_impl(&cwd, target).await {
        Ok(payload) => payload,
        Err(err) => TaskPickerPayload::Error {
            message: err.to_string(),
        },
    }
}

async fn load_task_picker_payload_impl(
    cwd: &Path,
    target: TaskPickerLoadTarget,
) -> anyhow::Result<TaskPickerPayload> {
    match resolve_load_target(cwd, target)? {
        ResolvedTarget::WorkspaceProjects { workspace } => {
            let projects = list_workspace_projects(&workspace).await?;
            Ok(TaskPickerPayload::ProjectList {
                workspace,
                projects,
            })
        }
        ResolvedTarget::Project { config } => load_project_tasks(cwd, config).await,
    }
}

enum ResolvedTarget {
    WorkspaceProjects { workspace: String },
    Project { config: ProjectConfig },
}

fn resolve_load_target(cwd: &Path, target: TaskPickerLoadTarget) -> anyhow::Result<ResolvedTarget> {
    match target {
        TaskPickerLoadTarget::Auto => {
            if let Some(config) = find_project_config_for_cwd(cwd)? {
                return Ok(ResolvedTarget::Project { config });
            }

            let global = GlobalConfig::load().unwrap_or_default();
            let workspace = global
                .plane_default_workspace
                .unwrap_or_else(|| DEFAULT_WORKSPACE.to_string());
            Ok(ResolvedTarget::WorkspaceProjects { workspace })
        }
        TaskPickerLoadTarget::Project {
            workspace,
            project_slug,
            project_name,
        } => Ok(ResolvedTarget::Project {
            config: ProjectConfig {
                workspace,
                project_slug,
                name: project_name,
                spec_files: Vec::new(),
            },
        }),
    }
}

fn find_project_config_for_cwd(cwd: &Path) -> anyhow::Result<Option<ProjectConfig>> {
    for ancestor in cwd.ancestors() {
        let config_path = ancestor.join(".palace").join("project.yml");
        if config_path.is_file() {
            let config = ProjectConfig::load(ancestor).with_context(|| {
                format!(
                    "Found {}, but failed to parse Palace project config",
                    config_path.display()
                )
            })?;
            return Ok(Some(config));
        }
    }
    Ok(None)
}

async fn load_project_tasks(
    cwd: &Path,
    config: ProjectConfig,
) -> anyhow::Result<TaskPickerPayload> {
    let project_name = config
        .name
        .clone()
        .unwrap_or_else(|| config.project_slug.clone());

    let client = PlaneClient::new().context("Failed to initialize Plane client")?;
    let issues = client.list_active_issues(&config).await.with_context(|| {
        format!(
            "Failed to list issues for {}/{}",
            config.workspace, config.project_slug
        )
    })?;

    let branch = codex_core::git_info::current_branch_name(cwd).await;
    let mut tasks = issues
        .into_iter()
        .map(|issue| issue_to_task_item(&config, issue, branch.clone()))
        .collect::<Vec<_>>();

    tasks.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));

    Ok(TaskPickerPayload::TaskList {
        workspace: config.workspace,
        project_slug: config.project_slug,
        project_name,
        tasks,
    })
}

fn issue_to_task_item(
    config: &ProjectConfig,
    issue: palace_plane::api::PlaneIssue,
    branch: Option<String>,
) -> TaskItem {
    let issue_ref = format!("{}-{}", config.project_slug, issue.sequence_id);
    let summary = summarize_issue_description(issue.description_html.as_deref())
        .unwrap_or_else(|| format!("Plane issue {issue_ref}"));
    let state = task_state_from_plane_state(issue.state.as_deref());

    let mut details = Vec::new();
    if let Some(priority) = issue
        .priority
        .as_deref()
        .filter(|priority| !priority.is_empty())
    {
        details.push(format!("Priority: {priority}"));
    }
    details.push(format!("Workspace: {}", config.workspace));
    details.push(format!("Project: {}", config.project_slug));

    TaskItem {
        id: format!("issue:{}", issue.id),
        title: issue.name.clone(),
        summary,
        details,
        prompt: format!(
            "Plan and implement Plane issue {issue_ref} ({}). Keep the work aligned with the issue intent, call out blockers, and summarize done vs open items.",
            issue.name
        ),
        source: TaskSource::Plane,
        state,
        branch,
        ci_status: None,
        plane_issue_id: Some(issue_ref),
        kind: TaskKind::RealIssue,
    }
}

fn summarize_issue_description(description_html: Option<&str>) -> Option<String> {
    let text = description_html.map(strip_html_tags)?;
    let compact = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if compact.is_empty() {
        None
    } else {
        Some(truncate_chars(&compact, 180))
    }
}

fn strip_html_tags(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;

    for ch in input.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            output.push(' ');
        } else if !in_tag {
            output.push(ch);
        }
    }

    output
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn task_state_from_plane_state(state: Option<&str>) -> TaskState {
    let Some(state) = state else {
        return TaskState::Ready;
    };

    let normalized = state.to_ascii_lowercase();
    if normalized.contains("block") {
        TaskState::Blocked
    } else if normalized.contains("progress") || normalized.contains("active") {
        TaskState::InProgress
    } else if normalized.contains("done") || normalized.contains("complete") {
        TaskState::Done
    } else {
        TaskState::Ready
    }
}

async fn list_workspace_projects(workspace: &str) -> anyhow::Result<Vec<TaskPickerProject>> {
    let global = GlobalConfig::load().context("Failed to load ~/.palace/config.yml")?;
    let api_key = global.plane_api_key().context(
        "Plane API key not configured (set PLANE_API_KEY or ~/.palace/credentials.json)",
    )?;
    let api_url = global.plane_url();

    let url = format!("{api_url}/workspaces/{workspace}/projects/");
    let response = reqwest::Client::new()
        .get(url)
        .header("X-API-Key", api_key)
        .header("Content-Type", "application/json")
        .send()
        .await
        .context("Failed to connect to Plane API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Plane API project list failed: {status} - {body}");
    }

    let parsed: PlaneProjectsResponse = response
        .json()
        .await
        .context("Failed to parse Plane project list response")?;

    let mut projects = parsed
        .results
        .into_iter()
        .map(|project| {
            let slug = if project.identifier.is_empty() {
                project.name.clone()
            } else {
                project.identifier
            };
            TaskPickerProject {
                workspace: workspace.to_string(),
                project_slug: slug,
                name: project.name,
                description: project.description,
            }
        })
        .collect::<Vec<_>>();

    projects.sort_by(|left, right| {
        left.project_slug
            .to_ascii_lowercase()
            .cmp(&right.project_slug.to_ascii_lowercase())
            .then(
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase()),
            )
    });

    Ok(projects)
}

#[derive(Debug, Deserialize)]
struct PlaneProjectsResponse {
    results: Vec<PlaneProjectResponse>,
}

#[derive(Debug, Deserialize)]
struct PlaneProjectResponse {
    #[serde(default)]
    identifier: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::TaskState;
    use super::task_state_from_plane_state;

    #[test]
    fn maps_plane_states_to_task_state() {
        assert_eq!(
            task_state_from_plane_state(Some("In Progress")),
            TaskState::InProgress
        );
        assert_eq!(
            task_state_from_plane_state(Some("Blocked")),
            TaskState::Blocked
        );
        assert_eq!(task_state_from_plane_state(Some("Done")), TaskState::Done);
        assert_eq!(task_state_from_plane_state(Some("Todo")), TaskState::Ready);
        assert_eq!(task_state_from_plane_state(None), TaskState::Ready);
    }
}
