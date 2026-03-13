use crate::app::AttentionMode;
use crate::line_truncation::line_width;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::renderable::Renderable;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectAttentionLevel {
    Approval,
    UserInput,
    Error,
}

impl ProjectAttentionLevel {
    pub(crate) fn marker(self) -> Span<'static> {
        match self {
            Self::Approval => "!".red().bold(),
            Self::UserInput => "?".cyan().bold(),
            Self::Error => "x".red().bold(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTabChromeEntry {
    pub(crate) label: String,
    pub(crate) attention: Option<ProjectAttentionLevel>,
    pub(crate) is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectTabsChromeState {
    pub(crate) workspace_custom_name: Option<String>,
    pub(crate) workspace_index: usize,
    pub(crate) workspace_count: usize,
    pub(crate) attention_mode: AttentionMode,
    pub(crate) tabs: Vec<ProjectTabChromeEntry>,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectTabsBar {
    state: Option<ProjectTabsChromeState>,
}

impl ProjectTabsBar {
    pub(crate) fn set_state(&mut self, state: Option<ProjectTabsChromeState>) {
        self.state = state;
    }

    fn is_relevant(state: &ProjectTabsChromeState) -> bool {
        state.tabs.len() > 1
            || state.workspace_count > 1
            || state.attention_mode == AttentionMode::On
            || state.tabs.iter().any(|tab| tab.attention.is_some())
    }

    fn tabs_line(state: &ProjectTabsChromeState) -> Line<'static> {
        let mut spans = Vec::new();

        for (idx, tab) in state.tabs.iter().enumerate() {
            if idx > 0 {
                spans.push("│".dim());
            }

            if let Some(attention) = tab.attention
                && state.attention_mode.shows_markers()
            {
                spans.push(attention.marker());
                spans.push(" ".into());
            }

            spans.push(if tab.is_active {
                tab.label.clone().cyan().bold()
            } else {
                tab.label.clone().dim()
            });
        }

        Line::from(spans)
    }

    fn summary_line(state: &ProjectTabsChromeState) -> Line<'static> {
        let mut spans = Vec::new();
        match state.attention_mode {
            AttentionMode::Off => {
                spans.push("// ".dim());
            }
            AttentionMode::Soft => {
                spans.push("/".dim());
                spans.push("A".red());
                spans.push("/ ".dim());
            }
            AttentionMode::On => {
                spans.push("/".dim());
                spans.push("A".red().bold());
                spans.push("/ ".dim());
            }
        }
        spans.push(
            format!(
                "workspace {}/{}",
                state.workspace_index + 1,
                state.workspace_count
            )
            .dim(),
        );
        if let Some(workspace_name) = state.workspace_custom_name.as_deref() {
            spans.push(format!(" ({workspace_name})").dim());
        }

        Line::from(spans)
    }

    fn render_line(state: &ProjectTabsChromeState, width: u16) -> Line<'static> {
        let summary = Self::summary_line(state);
        let width = usize::from(width);
        let summary_width = line_width(&summary);
        if summary_width >= width {
            return truncate_line_with_ellipsis_if_overflow(summary, width);
        }

        let tabs = truncate_line_with_ellipsis_if_overflow(
            Self::tabs_line(state),
            width.saturating_sub(summary_width + 1),
        );
        let mut spans = tabs.spans;
        if !spans.is_empty() {
            spans.push(" ".into());
        }
        spans.extend(summary.spans);

        Line::from(spans)
    }

    pub(crate) fn render_line_for_width(&self, width: u16) -> Option<Line<'static>> {
        let state = self.state.as_ref()?;
        Self::is_relevant(state).then(|| Self::render_line(state, width))
    }
}

impl Renderable for ProjectTabsBar {
    fn desired_height(&self, _width: u16) -> u16 {
        self.state
            .as_ref()
            .map_or(0, |state| u16::from(Self::is_relevant(state)))
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if area.is_empty() || !Self::is_relevant(state) {
            return;
        }
        Paragraph::new(Self::render_line(state, area.width)).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectAttentionLevel;
    use super::ProjectTabChromeEntry;
    use super::ProjectTabsBar;
    use super::ProjectTabsChromeState;
    use crate::app::AttentionMode;
    use crate::render::renderable::Renderable;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn render_tabs_bar(state: ProjectTabsChromeState, width: u16) -> String {
        let mut bar = ProjectTabsBar::default();
        bar.set_state(Some(state));
        let area = Rect::new(0, 0, width, bar.desired_height(width));
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);

        (0..area.width)
            .map(|col| {
                let symbol = buf[(area.x + col, area.y)].symbol();
                if symbol.is_empty() {
                    ' '
                } else {
                    symbol.chars().next().unwrap_or(' ')
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn sample_state() -> ProjectTabsChromeState {
        ProjectTabsChromeState {
            workspace_custom_name: Some("Riff Labs".to_string()),
            workspace_index: 0,
            workspace_count: 3,
            attention_mode: AttentionMode::On,
            tabs: vec![
                ProjectTabChromeEntry {
                    label: "obsidian".to_string(),
                    attention: None,
                    is_active: false,
                },
                ProjectTabChromeEntry {
                    label: "riff-connect".to_string(),
                    attention: Some(ProjectAttentionLevel::UserInput),
                    is_active: true,
                },
                ProjectTabChromeEntry {
                    label: "riff-environment".to_string(),
                    attention: Some(ProjectAttentionLevel::Approval),
                    is_active: false,
                },
                ProjectTabChromeEntry {
                    label: "codex".to_string(),
                    attention: None,
                    is_active: false,
                },
            ],
        }
    }

    #[test]
    fn project_tabs_bar_renders_single_line_summary() {
        assert_snapshot!(
            "project_tabs_bar_single_line",
            render_tabs_bar(sample_state(), 84)
        );
    }

    #[test]
    fn project_tabs_bar_truncates_tabs_before_summary() {
        assert_snapshot!(
            "project_tabs_bar_single_line_narrow",
            render_tabs_bar(sample_state(), 48)
        );
    }

    #[test]
    fn project_tabs_bar_collapses_attention_delimiter_when_attention_is_off() {
        let mut state = sample_state();
        state.attention_mode = AttentionMode::Off;

        assert_eq!(
            render_tabs_bar(state, 84),
            "obsidian│riff-connect│riff-environment│codex // workspace 1/3 (Riff Labs)"
        );
    }

    #[test]
    fn project_tabs_bar_hides_when_not_relevant() {
        let state = ProjectTabsChromeState {
            workspace_custom_name: None,
            workspace_index: 0,
            workspace_count: 1,
            attention_mode: AttentionMode::Off,
            tabs: vec![ProjectTabChromeEntry {
                label: "codex".to_string(),
                attention: None,
                is_active: true,
            }],
        };

        let mut bar = ProjectTabsBar::default();
        bar.set_state(Some(state));
        assert_eq!(bar.desired_height(80), 0);
    }

    #[test]
    fn project_tabs_bar_hides_markers_when_attention_is_off() {
        let rendered = render_tabs_bar(
            ProjectTabsChromeState {
                workspace_custom_name: None,
                workspace_index: 0,
                workspace_count: 1,
                attention_mode: AttentionMode::Off,
                tabs: vec![
                    ProjectTabChromeEntry {
                        label: "obsidian".to_string(),
                        attention: Some(ProjectAttentionLevel::UserInput),
                        is_active: true,
                    },
                    ProjectTabChromeEntry {
                        label: "codex".to_string(),
                        attention: Some(ProjectAttentionLevel::Approval),
                        is_active: false,
                    },
                ],
            },
            80,
        );

        assert_eq!(rendered.contains('?'), false);
        assert_eq!(rendered.contains('!'), false);
        assert_eq!(rendered.contains('A'), false);
    }
}
