use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::app_event::AppEvent;
use crate::app_event::ProjectOpenTarget;
use crate::app_event::ProjectTabPlacement;
use crate::app_event::SplitAxis;
use crate::app_event::SplitPaneTarget;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::CancellationEvent;
use crate::render::renderable::Renderable;
use crate::status::format_directory_display;
use crate::tui::GamepadAction;
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

use super::project_switcher::FavoriteProjectTile;

const PROJECT_CHOOSER_VIEW_ID: &str = "project-chooser";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectChooserPane {
    Favorites,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowserItem {
    Recent(PathBuf),
    Parent(PathBuf),
    Child(PathBuf),
}

pub(crate) struct ProjectChooserView {
    app_event_tx: AppEventSender,
    complete: bool,
    target: ProjectOpenTarget,
    favorites: Vec<FavoriteProjectTile>,
    recents: Vec<PathBuf>,
    browser_root: PathBuf,
    browser_items: Vec<BrowserItem>,
    focused_pane: ProjectChooserPane,
    selected_favorite_idx: usize,
    selected_browser_idx: usize,
}

impl ProjectChooserView {
    pub(crate) fn new(
        app_event_tx: AppEventSender,
        favorites: Vec<FavoriteProjectTile>,
        recents: Vec<PathBuf>,
        initial_root: Option<PathBuf>,
        target: ProjectOpenTarget,
    ) -> Self {
        let browser_root = preferred_browser_root(initial_root);
        let browser_items = build_browser_items(&browser_root, &recents);
        Self {
            app_event_tx,
            complete: false,
            target,
            favorites,
            recents,
            browser_root,
            browser_items,
            focused_pane: ProjectChooserPane::Favorites,
            selected_favorite_idx: 0,
            selected_browser_idx: 0,
        }
    }

    fn target_title(&self) -> &'static str {
        match self.target {
            ProjectOpenTarget::Tab(ProjectTabPlacement::Left) => "Open Project In LEFT Tab",
            ProjectOpenTarget::Tab(ProjectTabPlacement::Right) => "Open Project In RIGHT Tab",
            ProjectOpenTarget::SplitPane(SplitPaneTarget {
                axis: SplitAxis::Horizontal,
                ..
            }) => "Open Project In New Horizontal Pane",
            ProjectOpenTarget::SplitPane(SplitPaneTarget {
                axis: SplitAxis::Vertical,
                ..
            }) => "Open Project In New Vertical Pane",
        }
    }

    fn current_browser_path(&self) -> Option<PathBuf> {
        match self.browser_items.get(self.selected_browser_idx) {
            Some(BrowserItem::Recent(path))
            | Some(BrowserItem::Parent(path))
            | Some(BrowserItem::Child(path)) => Some(path.clone()),
            None => None,
        }
    }

    fn rebuild_browser(&mut self, root: PathBuf) {
        self.browser_root = root;
        self.browser_items = build_browser_items(&self.browser_root, &self.recents);
        self.selected_browser_idx = 0;
    }

    fn open_project(&mut self, cwd: PathBuf) {
        self.app_event_tx
            .send(AppEvent::FocusOrOpenProjectAtTarget {
                cwd,
                target: self.target,
            });
        self.complete = true;
    }

    fn open_new_session(&mut self, cwd: PathBuf) {
        self.app_event_tx
            .send(AppEvent::OpenNewProjectSessionAtTarget {
                cwd,
                target: self.target,
            });
        self.complete = true;
    }

    fn confirm(&mut self) {
        match self.focused_pane {
            ProjectChooserPane::Favorites => {
                if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                    self.open_project(tile.cwd.clone());
                }
            }
            ProjectChooserPane::Browser => {
                if let Some(path) = self.current_browser_path() {
                    self.open_project(path);
                }
            }
        }
    }

    fn browse_deeper(&mut self) {
        if self.focused_pane != ProjectChooserPane::Browser {
            return;
        }
        if let Some(path) = self.current_browser_path()
            && path.is_dir()
        {
            self.rebuild_browser(path);
        }
    }
}

impl BottomPaneView for ProjectChooserView {
    fn is_complete(&self) -> bool {
        self.complete
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(PROJECT_CHOOSER_VIEW_ID)
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
            } => self.focused_pane = ProjectChooserPane::Favorites,
            KeyEvent {
                code: KeyCode::Right,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.focused_pane == ProjectChooserPane::Browser {
                    self.browse_deeper();
                } else {
                    self.focused_pane = ProjectChooserPane::Browser;
                }
            }
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => match self.focused_pane {
                ProjectChooserPane::Favorites => {
                    self.selected_favorite_idx = self.selected_favorite_idx.saturating_sub(1);
                }
                ProjectChooserPane::Browser => {
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
                ProjectChooserPane::Favorites => {
                    if self.selected_favorite_idx + 1 < self.favorites.len() {
                        self.selected_favorite_idx += 1;
                    }
                }
                ProjectChooserPane::Browser => {
                    if self.selected_browser_idx + 1 < self.browser_items.len() {
                        self.selected_browser_idx += 1;
                    }
                }
            },
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.confirm(),
            KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                ..
            } => match self.focused_pane {
                ProjectChooserPane::Favorites => {
                    if let Some(tile) = self.favorites.get(self.selected_favorite_idx) {
                        self.open_new_session(tile.cwd.clone());
                    }
                }
                ProjectChooserPane::Browser => {
                    if let Some(path) = self.current_browser_path() {
                        self.open_new_session(path);
                    }
                }
            },
            KeyEvent {
                code: KeyCode::Esc, ..
            } => self.complete = true,
            _ => {}
        }
    }
}

impl Renderable for ProjectChooserView {
    fn desired_height(&self, _width: u16) -> u16 {
        match self.target {
            ProjectOpenTarget::Tab(_) => 26,
            ProjectOpenTarget::SplitPane(_) => 34,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        Clear.render(area, buf);
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(area);
        let [favorites_area, browser_area] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(body_area);

        Paragraph::new(vec![
            Line::from(self.target_title()).bold(),
            Line::from(
                "Choose a favorite, or browse into a directory. Enter opens; X starts fresh.".dim(),
            ),
            Line::from(format_directory_display(&self.browser_root, None).dim()),
        ])
        .render(header_area, buf);

        let favorite_lines = if self.favorites.is_empty() {
            vec!["No favorites yet.".into()]
        } else {
            self.favorites
                .iter()
                .enumerate()
                .map(|(idx, favorite)| {
                    let prefix = if self.focused_pane == ProjectChooserPane::Favorites
                        && self.selected_favorite_idx == idx
                    {
                        "> ".cyan().bold()
                    } else {
                        "  ".into()
                    };
                    let mut spans = vec![prefix];
                    if favorite.is_open {
                        spans.push("● ".magenta().bold());
                    }
                    spans.push(favorite.label.clone().into());
                    Line::from(spans)
                })
                .collect()
        };

        let favorites_block = Block::default()
            .borders(Borders::ALL)
            .border_type(if self.focused_pane == ProjectChooserPane::Favorites {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .title("Favorites")
            .border_style(if self.focused_pane == ProjectChooserPane::Favorites {
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
                    let prefix = if self.focused_pane == ProjectChooserPane::Browser
                        && self.selected_browser_idx == idx
                    {
                        "> ".magenta().bold()
                    } else {
                        "  ".into()
                    };
                    let text = match item {
                        BrowserItem::Recent(path) => {
                            format!("Recent: {}", format_directory_display(path, None))
                        }
                        BrowserItem::Parent(path) => {
                            format!(".. {}", format_directory_display(path, None))
                        }
                        BrowserItem::Child(path) => format_directory_display(path, None),
                    };
                    Line::from(vec![prefix, text.into()])
                })
                .collect()
        };

        let browser_block = Block::default()
            .borders(Borders::ALL)
            .border_type(if self.focused_pane == ProjectChooserPane::Browser {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .title(format!(
                "Browser: {}",
                format_directory_display(&self.browser_root, None)
            ))
            .border_style(if self.focused_pane == ProjectChooserPane::Browser {
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

        Paragraph::new(vec![
            Line::from("enter to open project | x to open fresh"),
            Line::from("right/l to browse directory | esc to cancel").dim(),
        ])
        .render(footer_area, buf);
    }
}

fn preferred_browser_root(initial_root: Option<PathBuf>) -> PathBuf {
    initial_root
        .or_else(|| {
            dirs::home_dir().and_then(|home| {
                let projects = home.join("projects");
                projects.is_dir().then_some(projects)
            })
        })
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn build_browser_items(root: &Path, recents: &[PathBuf]) -> Vec<BrowserItem> {
    let mut items = recents
        .iter()
        .take(6)
        .filter(|path| path.is_dir())
        .cloned()
        .map(BrowserItem::Recent)
        .collect::<Vec<_>>();
    if let Some(parent) = root.parent() {
        items.push(BrowserItem::Parent(parent.to_path_buf()));
    }
    if let Ok(entries) = list_child_directories(root) {
        items.extend(entries.into_iter().map(BrowserItem::Child));
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

#[cfg(test)]
mod tests {
    use super::ProjectChooserView;
    use crate::app_event::AppEvent;
    use crate::app_event::ProjectOpenTarget;
    use crate::app_event::ProjectTabPlacement;
    use crate::app_event::SplitAxis;
    use crate::app_event::SplitPaneTarget;
    use crate::app_event_sender::AppEventSender;
    use crate::bottom_pane::BottomPaneView;
    use crate::bottom_pane::FavoriteProjectTile;
    use crate::render::renderable::Renderable;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::path::Path;
    use std::path::PathBuf;
    use tokio::sync::mpsc::unbounded_channel;

    fn render_snapshot(view: &ProjectChooserView, width: u16) -> String {
        let height = view.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        let mut lines = Vec::new();
        for y in 0..height {
            let mut row = String::new();
            for x in 0..width {
                row.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            lines.push(row);
        }
        lines.join("\n")
    }

    fn make_view() -> (
        ProjectChooserView,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) {
        let (tx, rx) = unbounded_channel();
        let view = ProjectChooserView::new(
            AppEventSender::new(tx),
            vec![FavoriteProjectTile {
                cwd: PathBuf::from("/workspace/codex"),
                label: "codex".to_string(),
                description: None,
                is_open: true,
            }],
            vec![PathBuf::from("/workspace/dragonfly")],
            Some(PathBuf::from("/workspace")),
            ProjectOpenTarget::Tab(ProjectTabPlacement::Right),
        );
        (view, rx)
    }

    #[test]
    fn project_chooser_confirm_focuses_or_opens_selected_favorite_with_placement() {
        let (mut view, mut rx) = make_view();

        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            rx.try_recv().expect("expected chooser event"),
            AppEvent::FocusOrOpenProjectAtTarget { cwd, target }
                if cwd == Path::new("/workspace/codex")
                    && target == ProjectOpenTarget::Tab(ProjectTabPlacement::Right)
        ));
        assert!(view.is_complete());
    }

    #[test]
    fn project_chooser_context_opens_fresh_session_for_selected_favorite() {
        let (mut view, mut rx) = make_view();

        view.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert!(matches!(
            rx.try_recv().expect("expected chooser event"),
            AppEvent::OpenNewProjectSessionAtTarget { cwd, target }
                if cwd == Path::new("/workspace/codex")
                    && target == ProjectOpenTarget::Tab(ProjectTabPlacement::Right)
        ));
        assert!(view.is_complete());
    }

    #[test]
    fn project_chooser_snapshot() {
        let (view, _rx) = make_view();
        assert_snapshot!("project_chooser", render_snapshot(&view, 80));
    }

    #[test]
    fn split_pane_project_chooser_snapshot() {
        let (tx, _rx) = unbounded_channel();
        let view = ProjectChooserView::new(
            AppEventSender::new(tx),
            vec![FavoriteProjectTile {
                cwd: PathBuf::from("/workspace/codex"),
                label: "codex".to_string(),
                description: None,
                is_open: true,
            }],
            vec![PathBuf::from("/workspace/dragonfly")],
            Some(PathBuf::from("/workspace")),
            ProjectOpenTarget::SplitPane(SplitPaneTarget {
                pane_id: 7,
                axis: SplitAxis::Horizontal,
            }),
        );
        assert_snapshot!("split_pane_project_chooser", render_snapshot(&view, 80));
    }
}
