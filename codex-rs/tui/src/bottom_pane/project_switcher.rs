use crate::app::AttentionMode;
use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::app_event::AppEvent;
use crate::app_event::ProjectOpenTarget;
use crate::app_event::ProjectTabPlacement;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::chatwidget::ProjectAttentionLevel;
use crate::render::renderable::Renderable;
use crate::status::format_directory_display;
use crate::tui::GamepadAction;
use codex_protocol::ThreadId;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

const PROJECT_SWITCHER_VIEW_ID: &str = "project-switcher-grid";
const FAVORITES_EDITOR_VIEW_ID: &str = "favorites-editor-grid";
const TILE_WIDTH: u16 = 28;
const TILE_HEIGHT: u16 = 5;
const TILE_GAP: u16 = 1;
const ACTION_TILE_WIDTH: u16 = 14;
const ACTION_TILE_HEIGHT: u16 = 3;
const SECTION_CHROME_HEIGHT: u16 = 2;
const HEADER_HEIGHT: u16 = 2;
const FOOTER_HEIGHT: u16 = 1;
const MIN_TABS_SECTION_HEIGHT: u16 = 12;
const MIN_FAVORITES_SECTION_HEIGHT: u16 = 8;
const MIN_ACTIONS_SECTION_HEIGHT: u16 = 3;
const LIST_SCROLL_STEP: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileGridLayout {
    columns: usize,
    rows: usize,
    tile_width: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSwitcherTabTile {
    pub(crate) thread_id: ThreadId,
    pub(crate) label: String,
    pub(crate) cwd: PathBuf,
    pub(crate) summary: Option<String>,
    pub(crate) is_active: bool,
    pub(crate) attention: Option<ProjectAttentionLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FavoriteProjectTile {
    pub(crate) cwd: PathBuf,
    pub(crate) label: String,
    pub(crate) description: Option<String>,
    pub(crate) is_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectSwitcherSection {
    Tabs,
    Favorites,
    Actions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectSwitcherAction {
    BrowseProjects,
    BrowseFilesystem,
    EditFavorites,
    RenameCurrentTab,
    ToggleAttentionMode,
    CloseCurrentTab,
    SaveAndQuit,
}

impl ProjectSwitcherAction {
    fn label(self, attention_mode: AttentionMode) -> String {
        match self {
            Self::BrowseProjects => "Browse projects".to_string(),
            Self::BrowseFilesystem => "Browse /".to_string(),
            Self::EditFavorites => "Edit favorites".to_string(),
            Self::RenameCurrentTab => "Rename tab".to_string(),
            Self::ToggleAttentionMode => format!("Attention {}", attention_mode.next().label()),
            Self::CloseCurrentTab => "Close tab".to_string(),
            Self::SaveAndQuit => "Save + quit".to_string(),
        }
    }
}

pub(crate) struct ProjectSwitcherView {
    app_event_tx: AppEventSender,
    complete: bool,
    workspace_name: String,
    workspace_index: usize,
    workspace_count: usize,
    attention_mode: AttentionMode,
    tabs: Vec<ProjectSwitcherTabTile>,
    favorites: Vec<FavoriteProjectTile>,
    current_cwd: PathBuf,
    selected_section: ProjectSwitcherSection,
    selected_tab_idx: usize,
    selected_favorite_idx: usize,
    selected_action_idx: usize,
    tab_columns: Cell<usize>,
    favorite_columns: Cell<usize>,
    action_columns: Cell<usize>,
}

impl ProjectSwitcherView {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        workspace_name: String,
        workspace_index: usize,
        workspace_count: usize,
        attention_mode: AttentionMode,
        tabs: Vec<ProjectSwitcherTabTile>,
        favorites: Vec<FavoriteProjectTile>,
        current_cwd: PathBuf,
    ) -> Self {
        let selected_tab_idx = tabs
            .iter()
            .position(|tab| tab.is_active)
            .unwrap_or_default();
        Self {
            app_event_tx,
            complete: false,
            workspace_name,
            workspace_index,
            workspace_count,
            attention_mode,
            tabs,
            favorites,
            current_cwd,
            selected_section: ProjectSwitcherSection::Tabs,
            selected_tab_idx,
            selected_favorite_idx: 0,
            selected_action_idx: 0,
            tab_columns: Cell::new(1),
            favorite_columns: Cell::new(1),
            action_columns: Cell::new(1),
        }
    }

    fn action_items(&self) -> [ProjectSwitcherAction; 7] {
        [
            ProjectSwitcherAction::BrowseProjects,
            ProjectSwitcherAction::BrowseFilesystem,
            ProjectSwitcherAction::EditFavorites,
            ProjectSwitcherAction::RenameCurrentTab,
            ProjectSwitcherAction::ToggleAttentionMode,
            ProjectSwitcherAction::CloseCurrentTab,
            ProjectSwitcherAction::SaveAndQuit,
        ]
    }

    fn focus_next_section(&mut self) {
        self.selected_section = match self.selected_section {
            ProjectSwitcherSection::Tabs => ProjectSwitcherSection::Favorites,
            ProjectSwitcherSection::Favorites => ProjectSwitcherSection::Actions,
            ProjectSwitcherSection::Actions => ProjectSwitcherSection::Tabs,
        };
    }

    fn focus_previous_section(&mut self) {
        self.selected_section = match self.selected_section {
            ProjectSwitcherSection::Tabs => ProjectSwitcherSection::Actions,
            ProjectSwitcherSection::Favorites => ProjectSwitcherSection::Tabs,
            ProjectSwitcherSection::Actions => ProjectSwitcherSection::Favorites,
        };
    }

    fn move_left(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                if self.selected_tab_idx > 0 {
                    self.selected_tab_idx -= 1;
                }
            }
            ProjectSwitcherSection::Favorites => {
                if self.selected_favorite_idx > 0 {
                    self.selected_favorite_idx -= 1;
                }
            }
            ProjectSwitcherSection::Actions => {
                if self.selected_action_idx > 0 {
                    self.selected_action_idx -= 1;
                }
            }
        }
    }

    fn move_right(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                if self.selected_tab_idx + 1 < self.tabs.len() {
                    self.selected_tab_idx += 1;
                }
            }
            ProjectSwitcherSection::Favorites => {
                if self.selected_favorite_idx + 1 < self.favorites.len() {
                    self.selected_favorite_idx += 1;
                }
            }
            ProjectSwitcherSection::Actions => {
                if self.selected_action_idx + 1 < self.action_items().len() {
                    self.selected_action_idx += 1;
                }
            }
        }
    }

    fn move_up(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                let columns = self.tab_columns.get().max(1);
                if self.selected_tab_idx >= columns {
                    self.selected_tab_idx -= columns;
                } else {
                    self.focus_previous_section();
                }
            }
            ProjectSwitcherSection::Favorites => {
                let columns = self.favorite_columns.get().max(1);
                if self.selected_favorite_idx >= columns {
                    self.selected_favorite_idx -= columns;
                } else {
                    self.focus_previous_section();
                }
            }
            ProjectSwitcherSection::Actions => {
                let columns = self.action_columns.get().max(1);
                if self.selected_action_idx >= columns {
                    self.selected_action_idx -= columns;
                } else {
                    self.focus_previous_section();
                }
            }
        }
    }

    fn move_down(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                let columns = self.tab_columns.get().max(1);
                if self.selected_tab_idx + columns < self.tabs.len() {
                    self.selected_tab_idx += columns;
                } else {
                    self.focus_next_section();
                }
            }
            ProjectSwitcherSection::Favorites => {
                let columns = self.favorite_columns.get().max(1);
                if self.selected_favorite_idx + columns < self.favorites.len() {
                    self.selected_favorite_idx += columns;
                } else {
                    self.focus_next_section();
                }
            }
            ProjectSwitcherSection::Actions => {
                let columns = self.action_columns.get().max(1);
                if self.selected_action_idx + columns < self.action_items().len() {
                    self.selected_action_idx += columns;
                } else {
                    self.focus_next_section();
                }
            }
        }
    }

    fn confirm(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                if let Some(tile) = self.tabs.get(self.selected_tab_idx) {
                    self.app_event_tx
                        .send(AppEvent::SelectAgentThread(tile.thread_id));
                    self.complete = true;
                }
            }
            ProjectSwitcherSection::Favorites => {
                if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                    self.app_event_tx.send(AppEvent::ResumeProjectAtTarget {
                        cwd: tile.cwd.clone(),
                        target: ProjectOpenTarget::Tab(ProjectTabPlacement::Right),
                    });
                    self.complete = true;
                }
            }
            ProjectSwitcherSection::Actions => self.run_action(),
        }
    }

    fn context_action(&mut self) {
        match self.selected_section {
            ProjectSwitcherSection::Tabs => {
                if let Some(tile) = self.tabs.get(self.selected_tab_idx) {
                    self.app_event_tx.send(AppEvent::CloseProjectTab {
                        thread_id: tile.thread_id,
                    });
                    self.complete = true;
                }
            }
            ProjectSwitcherSection::Favorites => {
                if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                    self.app_event_tx
                        .send(AppEvent::OpenNewProjectSessionAtTarget {
                            cwd: tile.cwd.clone(),
                            target: ProjectOpenTarget::Tab(ProjectTabPlacement::Right),
                        });
                    self.complete = true;
                }
            }
            ProjectSwitcherSection::Actions => self.run_action(),
        }
    }

    fn run_action(&mut self) {
        let Some(action) = self.action_items().get(self.selected_action_idx).copied() else {
            return;
        };
        match action {
            ProjectSwitcherAction::BrowseProjects => {
                if let Some(projects_root) = dirs::home_dir().map(|home| home.join("projects")) {
                    self.app_event_tx
                        .send(AppEvent::OpenProjectDirectoryBrowser {
                            root: projects_root,
                        });
                }
            }
            ProjectSwitcherAction::BrowseFilesystem => {
                self.app_event_tx
                    .send(AppEvent::OpenProjectDirectoryBrowser {
                        root: PathBuf::from("/"),
                    });
            }
            ProjectSwitcherAction::EditFavorites => {
                self.app_event_tx
                    .send(AppEvent::OpenProjectFavoritesManager { initial_path: None });
            }
            ProjectSwitcherAction::RenameCurrentTab => {
                self.app_event_tx.send(AppEvent::OpenRenameCurrentTabPrompt);
            }
            ProjectSwitcherAction::ToggleAttentionMode => {
                self.app_event_tx.send(AppEvent::SetAttentionMode {
                    mode: self.attention_mode.next(),
                });
            }
            ProjectSwitcherAction::CloseCurrentTab => {
                if let Some(tab) = self.tabs.iter().find(|tab| tab.is_active) {
                    self.app_event_tx.send(AppEvent::CloseProjectTab {
                        thread_id: tab.thread_id,
                    });
                }
            }
            ProjectSwitcherAction::SaveAndQuit => {
                self.app_event_tx.send(AppEvent::SaveWorkspaceAndExit);
            }
        }
        self.complete = true;
    }

    fn grid_layout(item_count: usize, area_width: u16, item_width: u16) -> TileGridLayout {
        let columns = usize::from(area_width.saturating_add(TILE_GAP) / item_width).max(1);
        let rows = item_count.max(1).div_ceil(columns);
        let tile_width = if columns == 1 {
            area_width.max(1)
        } else {
            item_width.saturating_sub(TILE_GAP)
        };

        TileGridLayout {
            columns,
            rows,
            tile_width,
        }
    }

    fn tile_grid_layout(item_count: usize, area_width: u16) -> TileGridLayout {
        Self::grid_layout(item_count, area_width, TILE_WIDTH)
    }

    fn tile_grid_section_height(item_count: usize, popup_width: u16, min_height: u16) -> u16 {
        let layout = Self::tile_grid_layout(item_count, popup_width.saturating_sub(2));
        let grid_height = SECTION_CHROME_HEIGHT + (layout.rows as u16) * TILE_HEIGHT;
        grid_height.max(min_height)
    }

    fn action_grid_layout(&self, area_width: u16) -> TileGridLayout {
        Self::grid_layout(self.action_items().len(), area_width, ACTION_TILE_WIDTH)
    }

    fn action_grid_section_height(&self, popup_width: u16) -> u16 {
        let layout = self.action_grid_layout(popup_width);
        ((layout.rows as u16) * ACTION_TILE_HEIGHT).max(MIN_ACTIONS_SECTION_HEIGHT)
    }

    fn render_tile_grid<T>(
        &self,
        area: Rect,
        buf: &mut Buffer,
        items: &[T],
        selected_idx: usize,
        selected_section: bool,
        columns_sink: &Cell<usize>,
        render_tile: impl Fn(&T, bool, Rect, &mut Buffer),
    ) {
        if area.is_empty() {
            columns_sink.set(1);
            return;
        }
        let layout = Self::tile_grid_layout(items.len(), area.width);
        columns_sink.set(layout.columns);
        let column_stride = layout.tile_width + u16::from(layout.columns > 1);
        for row in 0..layout.rows {
            for col in 0..layout.columns {
                let idx = row * layout.columns + col;
                if idx >= items.len() {
                    break;
                }
                let x = area.x + (col as u16) * column_stride;
                let y = area.y + (row as u16) * TILE_HEIGHT;
                if y + TILE_HEIGHT > area.bottom() || x >= area.right() {
                    continue;
                }
                let tile_area =
                    Rect::new(x, y, layout.tile_width.min(area.right() - x), TILE_HEIGHT);
                render_tile(
                    &items[idx],
                    selected_section && idx == selected_idx,
                    tile_area,
                    buf,
                );
            }
        }
    }

    fn render_action_grid(&self, area: Rect, buf: &mut Buffer) {
        let actions = self.action_items();
        if area.is_empty() {
            self.action_columns.set(1);
            return;
        }
        let layout = self.action_grid_layout(area.width);
        self.action_columns.set(layout.columns);
        let column_stride = layout.tile_width + u16::from(layout.columns > 1);
        for row in 0..layout.rows {
            for col in 0..layout.columns {
                let idx = row * layout.columns + col;
                let Some(action) = actions.get(idx) else {
                    break;
                };
                let x = area.x + (col as u16) * column_stride;
                let y = area.y + (row as u16) * ACTION_TILE_HEIGHT;
                if y + ACTION_TILE_HEIGHT > area.bottom() || x >= area.right() {
                    continue;
                }
                let selected = self.selected_section == ProjectSwitcherSection::Actions
                    && self.selected_action_idx == idx;
                let border_style = if selected {
                    Style::default().cyan().bold()
                } else {
                    Style::default().dim()
                };
                Paragraph::new(Line::from(action.label(self.attention_mode)))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(if selected {
                                BorderType::Double
                            } else {
                                BorderType::Plain
                            })
                            .border_style(border_style),
                    )
                    .render(
                        Rect::new(
                            x,
                            y,
                            layout.tile_width.min(area.right() - x),
                            ACTION_TILE_HEIGHT,
                        ),
                        buf,
                    );
            }
        }
    }
}

impl BottomPaneView for ProjectSwitcherView {
    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(PROJECT_SWITCHER_VIEW_ID)
    }

    fn map_gamepad_action(&self, action: GamepadAction) -> Option<KeyCode> {
        match action {
            GamepadAction::Confirm => Some(KeyCode::Enter),
            GamepadAction::Context => Some(KeyCode::Char('x')),
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
                code: KeyCode::Left,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_left(),
            KeyEvent {
                code: KeyCode::Right,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_right(),
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
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
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.confirm(),
            KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.context_action(),
            KeyEvent {
                code: KeyCode::Esc, ..
            } => self.complete = true,
            _ => {}
        }
    }
}

impl Renderable for ProjectSwitcherView {
    fn desired_height(&self, width: u16) -> u16 {
        HEADER_HEIGHT
            + Self::tile_grid_section_height(self.tabs.len(), width, MIN_TABS_SECTION_HEIGHT)
            + Self::tile_grid_section_height(
                self.favorites.len(),
                width,
                MIN_FAVORITES_SECTION_HEIGHT,
            )
            + self.action_grid_section_height(width)
            + FOOTER_HEIGHT
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        Clear.render(area, buf);
        let tabs_height =
            Self::tile_grid_section_height(self.tabs.len(), area.width, MIN_TABS_SECTION_HEIGHT);
        let favorites_height = Self::tile_grid_section_height(
            self.favorites.len(),
            area.width,
            MIN_FAVORITES_SECTION_HEIGHT,
        );
        let actions_height = self.action_grid_section_height(area.width);
        let [
            header_area,
            tabs_area,
            favorites_area,
            actions_area,
            footer_area,
        ] = Layout::vertical([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Length(tabs_height),
            Constraint::Length(favorites_height),
            Constraint::Length(actions_height),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .areas(area);
        let header = vec![
            Line::from(vec![
                "Project Switcher".bold(),
                "  ".into(),
                format!(
                    "{} ({}/{})",
                    self.workspace_name,
                    self.workspace_index + 1,
                    self.workspace_count
                )
                .dim(),
            ]),
            Line::from(vec![
                format_directory_display(&self.current_cwd, None).dim(),
                "  ".into(),
                format!("Attention {}", self.attention_mode.label()).dim(),
            ]),
        ];
        Paragraph::new(header).render(header_area, buf);

        let tabs_block = Block::default()
            .borders(Borders::ALL)
            .border_type(if self.selected_section == ProjectSwitcherSection::Tabs {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .title("Open Tabs")
            .border_style(if self.selected_section == ProjectSwitcherSection::Tabs {
                Style::default().cyan().bold()
            } else {
                Style::default()
            });
        let favorites_block = Block::default()
            .borders(Borders::ALL)
            .border_type(
                if self.selected_section == ProjectSwitcherSection::Favorites {
                    BorderType::Double
                } else {
                    BorderType::Plain
                },
            )
            .title("Favorites")
            .border_style(
                if self.selected_section == ProjectSwitcherSection::Favorites {
                    Style::default().magenta().bold()
                } else {
                    Style::default()
                },
            );
        let tabs_inner = tabs_block.inner(tabs_area);
        let favorites_inner = favorites_block.inner(favorites_area);
        tabs_block.render(tabs_area, buf);
        favorites_block.render(favorites_area, buf);

        self.render_tile_grid(
            tabs_inner,
            buf,
            &self.tabs,
            self.selected_tab_idx,
            self.selected_section == ProjectSwitcherSection::Tabs,
            &self.tab_columns,
            |tile, selected, tile_area, tile_buf| {
                let border_style = if selected {
                    Style::default().cyan().bold()
                } else if tile.is_active {
                    Style::default().cyan()
                } else {
                    Style::default().dim()
                };
                let summary = tile
                    .summary
                    .clone()
                    .unwrap_or_else(|| "Ready".to_string())
                    .chars()
                    .take(140)
                    .collect::<String>();
                let mut title = Vec::new();
                if let Some(attention) = tile.attention {
                    title.push(attention.marker());
                    title.push(" ".into());
                }
                title.push(if tile.is_active {
                    tile.label.clone().cyan().bold()
                } else {
                    tile.label.clone().into()
                });
                Paragraph::new(vec![
                    Line::from(title),
                    Line::from(format_directory_display(&tile.cwd, None).dim()),
                    Line::from(summary.dim()),
                ])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(if selected {
                            BorderType::Double
                        } else {
                            BorderType::Plain
                        })
                        .border_style(border_style),
                )
                .render(tile_area, tile_buf);
            },
        );

        self.render_tile_grid(
            favorites_inner,
            buf,
            &self.favorites,
            self.selected_favorite_idx,
            self.selected_section == ProjectSwitcherSection::Favorites,
            &self.favorite_columns,
            |tile, selected, tile_area, tile_buf| {
                let border_style = if selected {
                    Style::default().magenta().bold()
                } else if tile.is_open {
                    Style::default().magenta()
                } else {
                    Style::default().dim()
                };
                let description = tile
                    .description
                    .clone()
                    .unwrap_or_else(|| "Press X to edit favorites".to_string());
                let title = if tile.is_open {
                    vec!["● ".magenta().bold(), tile.label.clone().magenta().bold()]
                } else {
                    vec![tile.label.clone().into()]
                };
                Paragraph::new(vec![
                    Line::from(title),
                    Line::from(format_directory_display(&tile.cwd, None).dim()),
                    Line::from(description.chars().take(140).collect::<String>().dim()),
                ])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(if selected {
                            BorderType::Double
                        } else {
                            BorderType::Plain
                        })
                        .border_style(border_style),
                )
                .render(tile_area, tile_buf);
            },
        );

        self.render_action_grid(actions_area, buf);
        Paragraph::new(standard_popup_hint_line()).render(footer_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::FavoriteProjectTile;
    use super::ProjectSwitcherView;
    use super::TileGridLayout;
    use crate::app::AttentionMode;
    use crate::app_event::AppEvent;
    use crate::app_event::ProjectOpenTarget;
    use crate::app_event::ProjectTabPlacement;
    use crate::app_event_sender::AppEventSender;
    use crate::bottom_pane::BottomPaneView;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::path::PathBuf;
    use tokio::sync::mpsc::unbounded_channel;

    fn make_view() -> (
        ProjectSwitcherView,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) {
        let (tx, rx) = unbounded_channel::<AppEvent>();
        let view = ProjectSwitcherView::new(
            AppEventSender::new(tx),
            "Workspace 1".to_string(),
            0,
            1,
            AttentionMode::Soft,
            Vec::new(),
            vec![FavoriteProjectTile {
                cwd: PathBuf::from("/workspace/flagship"),
                label: "flagship".to_string(),
                description: None,
                is_open: false,
            }],
            PathBuf::from("/workspace/codex"),
        );
        (view, rx)
    }

    #[test]
    fn tile_grid_layout_uses_exact_fit_columns() {
        assert_eq!(
            ProjectSwitcherView::tile_grid_layout(4, 55),
            TileGridLayout {
                columns: 2,
                rows: 2,
                tile_width: 27,
            }
        );
    }

    #[test]
    fn tile_grid_layout_shrinks_single_column_tiles() {
        assert_eq!(
            ProjectSwitcherView::tile_grid_layout(3, 20),
            TileGridLayout {
                columns: 1,
                rows: 3,
                tile_width: 20,
            }
        );
    }

    #[test]
    fn action_grid_layout_wraps_when_width_is_narrow() {
        let (view, _rx) = make_view();

        assert_eq!(
            view.action_grid_layout(58),
            TileGridLayout {
                columns: 4,
                rows: 2,
                tile_width: 13,
            }
        );
    }

    #[test]
    fn confirm_on_favorite_opens_resume_picker_for_project() {
        let (mut view, mut rx) = make_view();

        view.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            rx.try_recv().expect("expected switcher event"),
            AppEvent::ResumeProjectAtTarget { cwd, target }
                if cwd == Path::new("/workspace/flagship")
                    && target == ProjectOpenTarget::Tab(ProjectTabPlacement::Right)
        ));
    }

    #[test]
    fn context_on_favorite_opens_fresh_session_for_project() {
        let (mut view, mut rx) = make_view();

        view.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        view.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert!(matches!(
            rx.try_recv().expect("expected switcher event"),
            AppEvent::OpenNewProjectSessionAtTarget { cwd, target }
                if cwd == Path::new("/workspace/flagship")
                    && target == ProjectOpenTarget::Tab(ProjectTabPlacement::Right)
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FavoritesPane {
    Favorites,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FavoritesBrowserItem {
    Recent(PathBuf),
    Parent(PathBuf),
    Child(PathBuf),
}

pub(crate) struct FavoritesEditorView {
    app_event_tx: AppEventSender,
    complete: bool,
    initial_path: Option<PathBuf>,
    favorites: Vec<FavoriteProjectTile>,
    recents: Vec<PathBuf>,
    browser_root: PathBuf,
    browser_items: Vec<FavoritesBrowserItem>,
    focused_pane: FavoritesPane,
    selected_favorite_idx: usize,
    selected_browser_idx: usize,
}

impl FavoritesEditorView {
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        favorites: Vec<FavoriteProjectTile>,
        recents: Vec<PathBuf>,
        initial_path: Option<PathBuf>,
    ) -> Self {
        let browser_root = initial_path
            .clone()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let browser_items = build_browser_items(&browser_root, &recents);
        Self {
            app_event_tx,
            complete: false,
            initial_path,
            favorites,
            recents,
            browser_root,
            browser_items,
            focused_pane: FavoritesPane::Favorites,
            selected_favorite_idx: 0,
            selected_browser_idx: 0,
        }
    }

    fn rebuild_browser(&mut self, root: PathBuf) {
        self.browser_root = root;
        self.browser_items = build_browser_items(&self.browser_root, &self.recents);
        self.selected_browser_idx = 0;
    }

    fn current_browser_path(&self) -> Option<PathBuf> {
        match self.browser_items.get(self.selected_browser_idx) {
            Some(FavoritesBrowserItem::Recent(path))
            | Some(FavoritesBrowserItem::Parent(path))
            | Some(FavoritesBrowserItem::Child(path)) => Some(path.clone()),
            None => None,
        }
    }

    fn toggle_current_favorite(&mut self) {
        match self.focused_pane {
            FavoritesPane::Favorites => {
                if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                    self.app_event_tx.send(AppEvent::ToggleFavoriteProject {
                        cwd: tile.cwd.clone(),
                    });
                    self.complete = true;
                }
            }
            FavoritesPane::Browser => {
                if let Some(path) = self.current_browser_path() {
                    self.app_event_tx
                        .send(AppEvent::ToggleFavoriteProject { cwd: path });
                    self.complete = true;
                }
            }
        }
    }

    fn open_current_project(&mut self) {
        match self.focused_pane {
            FavoritesPane::Favorites => {
                if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                    self.app_event_tx.send(AppEvent::FocusOrOpenProject {
                        cwd: tile.cwd.clone(),
                    });
                    self.complete = true;
                }
            }
            FavoritesPane::Browser => {
                if let Some(path) = self.current_browser_path() {
                    self.app_event_tx
                        .send(AppEvent::FocusOrOpenProject { cwd: path });
                    self.complete = true;
                }
            }
        }
    }
}

impl BottomPaneView for FavoritesEditorView {
    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(FAVORITES_EDITOR_VIEW_ID)
    }

    fn map_gamepad_action(&self, action: GamepadAction) -> Option<KeyCode> {
        match action {
            GamepadAction::Confirm => Some(KeyCode::Enter),
            GamepadAction::Context => Some(KeyCode::Char('x')),
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
                code: KeyCode::Left,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.focused_pane = FavoritesPane::Favorites,
            KeyEvent {
                code: KeyCode::Right,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.focused_pane = FavoritesPane::Browser,
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => match self.focused_pane {
                FavoritesPane::Favorites => {
                    self.selected_favorite_idx = self.selected_favorite_idx.saturating_sub(1);
                }
                FavoritesPane::Browser => {
                    self.selected_browser_idx = self.selected_browser_idx.saturating_sub(1);
                }
            },
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => match self.focused_pane {
                FavoritesPane::Favorites => {
                    if self.selected_favorite_idx + 1 < self.favorites.len() {
                        self.selected_favorite_idx += 1;
                    }
                }
                FavoritesPane::Browser => {
                    if self.selected_browser_idx + 1 < self.browser_items.len() {
                        self.selected_browser_idx += 1;
                    }
                }
            },
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => {
                if self.focused_pane == FavoritesPane::Browser
                    && let Some(path) = self.current_browser_path()
                    && path.is_dir()
                {
                    self.rebuild_browser(path);
                } else {
                    self.open_current_project();
                }
            }
            KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.toggle_current_favorite(),
            KeyEvent {
                code: KeyCode::Esc, ..
            } => self.complete = true,
            _ => {}
        }
    }
}

impl Renderable for FavoritesEditorView {
    fn desired_height(&self, _width: u16) -> u16 {
        20
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        Clear.render(area, buf);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let [favorites_area, browser_area] =
            Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
                .areas(body_area);
        Paragraph::new(vec![
            Line::from("Edit Favorites".bold()),
            Line::from(
                self.initial_path
                    .as_ref()
                    .map(|path| format_directory_display(path, None))
                    .unwrap_or_else(|| "Use left/right to switch panes".to_string())
                    .dim(),
            ),
        ])
        .render(header_area, buf);

        let favorite_lines = if self.favorites.is_empty() {
            vec!["No favorites yet.".into()]
        } else {
            self.favorites
                .iter()
                .enumerate()
                .map(|(idx, favorite)| {
                    let prefix = if self.focused_pane == FavoritesPane::Favorites
                        && self.selected_favorite_idx == idx
                    {
                        "> ".cyan().bold()
                    } else {
                        "  ".into()
                    };
                    Line::from(vec![prefix, favorite.label.clone().into()])
                })
                .collect()
        };
        let favorites_block = Block::default()
            .borders(Borders::ALL)
            .title("Favorites")
            .border_style(if self.focused_pane == FavoritesPane::Favorites {
                Style::default().cyan().bold()
            } else {
                Style::default()
            });
        let favorites_inner = favorites_block.inner(favorites_area);
        favorites_block.render(favorites_area, buf);
        Paragraph::new(favorite_lines)
            .scroll((
                u16::try_from(list_scroll_top(
                    self.selected_favorite_idx,
                    self.favorites.len(),
                    favorites_inner.height as usize,
                ))
                .unwrap_or(u16::MAX),
                0,
            ))
            .render(favorites_inner, buf);

        let browser_lines = if self.browser_items.is_empty() {
            vec!["No browser items.".into()]
        } else {
            self.browser_items
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let prefix = if self.focused_pane == FavoritesPane::Browser
                        && self.selected_browser_idx == idx
                    {
                        "> ".magenta().bold()
                    } else {
                        "  ".into()
                    };
                    let text = match item {
                        FavoritesBrowserItem::Recent(path) => {
                            format!("Recent: {}", format_directory_display(path, None))
                        }
                        FavoritesBrowserItem::Parent(path) => {
                            format!(".. {}", format_directory_display(path, None))
                        }
                        FavoritesBrowserItem::Child(path) => format_directory_display(path, None),
                    };
                    Line::from(vec![prefix, text.into()])
                })
                .collect()
        };
        let browser_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                "Browser: {}",
                format_directory_display(&self.browser_root, None)
            ))
            .border_style(if self.focused_pane == FavoritesPane::Browser {
                Style::default().magenta().bold()
            } else {
                Style::default()
            });
        let browser_inner = browser_block.inner(browser_area);
        browser_block.render(browser_area, buf);
        Paragraph::new(browser_lines)
            .scroll((
                u16::try_from(list_scroll_top(
                    self.selected_browser_idx,
                    self.browser_items.len(),
                    browser_inner.height as usize,
                ))
                .unwrap_or(u16::MAX),
                0,
            ))
            .render(browser_inner, buf);

        Paragraph::new(standard_popup_hint_line()).render(footer_area, buf);
    }
}

fn build_browser_items(root: &Path, recents: &[PathBuf]) -> Vec<FavoritesBrowserItem> {
    let mut items = recents
        .iter()
        .take(6)
        .cloned()
        .map(FavoritesBrowserItem::Recent)
        .collect::<Vec<_>>();
    if let Some(parent) = root.parent() {
        items.push(FavoritesBrowserItem::Parent(parent.to_path_buf()));
    }
    if let Ok(entries) = list_child_directories(root) {
        items.extend(entries.into_iter().map(FavoritesBrowserItem::Child));
    }
    items
}

fn list_child_directories(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut children = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir().then_some(path)
        })
        .collect::<Vec<_>>();
    children.sort();
    Ok(children)
}

fn list_scroll_top(selected_idx: usize, len: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 || len <= visible_rows {
        0
    } else {
        selected_idx
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(len.saturating_sub(visible_rows))
    }
}

fn scroll_selection(selected_idx: &mut usize, len: usize, delta: i32) {
    if len == 0 {
        *selected_idx = 0;
        return;
    }
    if delta >= 0 {
        *selected_idx = selected_idx
            .saturating_add(delta as usize)
            .min(len.saturating_sub(1));
    } else {
        *selected_idx = selected_idx.saturating_sub(delta.unsigned_abs() as usize);
    }
}
