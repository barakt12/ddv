use ratatui::{
    crossterm::event::KeyEvent, layout::Rect, style::Style, text::Line, widgets::ListItem, Frame,
};

use crate::{
    color::ColorTheme,
    event::{AppEvent, Sender, UserEvent, UserEventMapper},
    handle_user_events,
    help::{
        build_help_spans, build_short_help_spans, BuildHelpsItem, BuildShortHelpsItem, Spans,
        SpansWithPriority,
    },
    widget::{ScrollList, ScrollListState},
};

pub struct ProfileListView {
    profiles: Vec<String>,
    list_state: ScrollListState,

    helps: Vec<Spans>,
    helps_short: Vec<SpansWithPriority>,
    theme: ColorTheme,
    tx: Sender,
}

impl ProfileListView {
    pub fn new(
        profiles: Vec<String>,
        mapper: &UserEventMapper,
        theme: ColorTheme,
        tx: Sender,
    ) -> Self {
        let list_state = ScrollListState::new(profiles.len());
        ProfileListView {
            profiles,
            list_state,
            helps: build_helps(mapper, theme),
            helps_short: build_short_helps(mapper),
            theme,
            tx,
        }
    }

    fn selected(&self) -> Option<&String> {
        self.profiles.get(self.list_state.selected)
    }
}

impl ProfileListView {
    pub fn handle_user_key_event(&mut self, user_events: Vec<UserEvent>, _key_event: KeyEvent) {
        handle_user_events! { user_events =>
            UserEvent::Down => {
                self.list_state.select_next();
            }
            UserEvent::Up => {
                self.list_state.select_prev();
            }
            UserEvent::PageDown => {
                self.list_state.select_next_page();
            }
            UserEvent::PageUp => {
                self.list_state.select_prev_page();
            }
            UserEvent::GoToTop => {
                self.list_state.select_first();
            }
            UserEvent::GoToBottom => {
                self.list_state.select_last();
            }
            UserEvent::Confirm => {
                if let Some(profile) = self.selected() {
                    self.tx.send(AppEvent::SelectProfile(profile.clone()));
                }
            }
            UserEvent::Help => {
                self.tx.send(AppEvent::OpenHelp(self.helps.clone()));
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .profiles
            .iter()
            .enumerate()
            .skip(self.list_state.offset)
            .map(|(i, name)| {
                let mut style = Style::default().fg(self.theme.fg).bg(self.theme.bg);
                if i == self.list_state.selected {
                    style = style.fg(self.theme.selected_fg).bg(self.theme.selected_bg);
                }
                ListItem::new(Line::from(format!(" {name}"))).style(style)
            })
            .collect();
        let list = ScrollList::new(items).theme(&self.theme).focused(true);
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    pub fn short_helps(&self) -> &[SpansWithPriority] {
        &self.helps_short
    }
}

fn build_helps(mapper: &UserEventMapper, theme: ColorTheme) -> Vec<Spans> {
    #[rustfmt::skip]
    let helps = vec![
        BuildHelpsItem::new(UserEvent::Quit, "Quit app"),
        BuildHelpsItem::new(UserEvent::Down, "Select next"),
        BuildHelpsItem::new(UserEvent::Up, "Select previous"),
        BuildHelpsItem::new(UserEvent::Confirm, "Use profile"),
    ];
    build_help_spans(helps, mapper, theme)
}

fn build_short_helps(mapper: &UserEventMapper) -> Vec<SpansWithPriority> {
    #[rustfmt::skip]
    let helps = vec![
        BuildShortHelpsItem::single(UserEvent::Quit, "Quit", 0),
        BuildShortHelpsItem::group(vec![UserEvent::Down, UserEvent::Up], "Select", 2),
        BuildShortHelpsItem::single(UserEvent::Confirm, "Use profile", 1),
    ];
    build_short_help_spans(helps, mapper)
}
