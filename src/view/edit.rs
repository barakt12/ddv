use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, Paragraph},
    Frame,
};
use tui_textarea::TextArea;

use crate::{
    color::ColorTheme,
    data::{Item, PlainJsonItem, RawJsonItem, TableDescription},
    event::{AppEvent, Sender, UserEvent, UserEventMapper},
    help::{
        build_help_spans, build_short_help_spans, BuildHelpsItem, BuildShortHelpsItem, Spans,
        SpansWithPriority,
    },
    item_json::{
        is_plain_convertible, item_from_plain_json, item_from_typed_json, new_item_skeleton,
        validate_has_keys,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonMode {
    Typed,
    Plain,
}

impl JsonMode {
    fn label(&self) -> &'static str {
        match self {
            JsonMode::Typed => "typed",
            JsonMode::Plain => "plain",
        }
    }
}

pub struct EditView {
    table_description: TableDescription,
    editing: bool, // true = editing an existing item, false = creating
    textarea: TextArea<'static>,
    mode: JsonMode,
    error: Option<String>,

    helps: Vec<Spans>,
    helps_short: Vec<SpansWithPriority>,
    theme: ColorTheme,
    tx: Sender,
}

impl EditView {
    pub fn new(
        table_description: TableDescription,
        item: Option<Item>,
        mapper: &UserEventMapper,
        theme: ColorTheme,
        tx: Sender,
    ) -> Self {
        let editing = item.is_some();
        let schema = &table_description.key_schema_type;
        let initial = item.unwrap_or_else(|| new_item_skeleton(schema));
        let json = serde_json::to_string_pretty(&RawJsonItem::new(&initial, schema))
            .unwrap_or_else(|_| "{}".to_string());
        let textarea = TextArea::new(json.lines().map(str::to_string).collect());

        EditView {
            table_description,
            editing,
            textarea,
            mode: JsonMode::Typed,
            error: None,
            helps: build_helps(mapper, theme),
            helps_short: build_short_helps(mapper),
            theme,
            tx,
        }
    }
}

impl EditView {
    pub fn handle_user_key_event(&mut self, user_events: Vec<UserEvent>, key_event: KeyEvent) {
        let has = |e: UserEvent| user_events.contains(&e);

        if has(UserEvent::Save) {
            self.save();
            return;
        }
        if has(UserEvent::ToggleJsonMode) {
            self.toggle_mode();
            return;
        }
        if has(UserEvent::Reset) {
            self.tx.send(AppEvent::BackToBeforeView);
            return;
        }
        // everything else is text editing
        self.textarea.input(key_event);
        self.error = None;
    }

    fn current_item(&self) -> Result<Item, String> {
        let text = self.textarea.lines().join("\n");
        match self.mode {
            JsonMode::Typed => item_from_typed_json(&text),
            JsonMode::Plain => item_from_plain_json(&text),
        }
    }

    fn save(&mut self) {
        let item = match self.current_item() {
            Ok(item) => item,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Err(e) = validate_has_keys(&item, &self.table_description.key_schema_type) {
            self.error = Some(e);
            return;
        }
        self.tx
            .send(AppEvent::SaveItem(self.table_description.clone(), item));
    }

    fn toggle_mode(&mut self) {
        let item = match self.current_item() {
            Ok(item) => item,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let target = match self.mode {
            JsonMode::Typed => JsonMode::Plain,
            JsonMode::Plain => JsonMode::Typed,
        };
        if target == JsonMode::Plain && !is_plain_convertible(&item) {
            self.error =
                Some("can't switch to plain: item has set/binary attributes".to_string());
            return;
        }
        let schema = &self.table_description.key_schema_type;
        let json = match target {
            JsonMode::Typed => serde_json::to_string_pretty(&RawJsonItem::new(&item, schema)),
            JsonMode::Plain => serde_json::to_string_pretty(&PlainJsonItem::new(&item, schema)),
        }
        .unwrap_or_else(|_| "{}".to_string());
        self.textarea = TextArea::new(json.lines().map(str::to_string).collect());
        self.mode = target;
        self.error = None;
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [editor_area, footer_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

        let verb = if self.editing { "Edit" } else { "New item" };
        let title = format!(
            " {} {}  [{}] ",
            verb,
            self.table_description.table_name,
            self.mode.label()
        );
        let block = Block::bordered()
            .title_top(Line::from(title).left_aligned())
            .fg(self.theme.fg)
            .bg(self.theme.bg);
        self.textarea.set_block(block);
        self.textarea
            .set_cursor_style(Style::default().fg(self.theme.bg).bg(self.theme.fg));
        f.render_widget(&self.textarea, editor_area);

        let footer = match &self.error {
            Some(err) => Line::from(format!("  {err}").fg(self.theme.notification_error).bold()),
            None => Line::from(
                "  C-s: save   C-t: typed/plain   Esc: cancel".fg(self.theme.short_help),
            ),
        };
        f.render_widget(Paragraph::new(footer), footer_area);
    }

    pub fn short_helps(&self) -> &[SpansWithPriority] {
        &self.helps_short
    }
}

fn build_helps(mapper: &UserEventMapper, theme: ColorTheme) -> Vec<Spans> {
    #[rustfmt::skip]
    let helps = vec![
        BuildHelpsItem::new(UserEvent::Quit, "Quit app"),
        BuildHelpsItem::new(UserEvent::Reset, "Cancel"),
        BuildHelpsItem::new(UserEvent::Save, "Save item"),
        BuildHelpsItem::new(UserEvent::ToggleJsonMode, "Toggle typed/plain JSON"),
    ];
    build_help_spans(helps, mapper, theme)
}

fn build_short_helps(mapper: &UserEventMapper) -> Vec<SpansWithPriority> {
    #[rustfmt::skip]
    let helps = vec![
        BuildShortHelpsItem::single(UserEvent::Quit, "Quit", 0),
        BuildShortHelpsItem::single(UserEvent::Reset, "Cancel", 1),
        BuildShortHelpsItem::single(UserEvent::Save, "Save", 0),
        BuildShortHelpsItem::single(UserEvent::ToggleJsonMode, "Typed/Plain", 2),
    ];
    build_short_help_spans(helps, mapper)
}
