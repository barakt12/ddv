use std::str::FromStr;

use ratatui::{
    crossterm::event::{Event, KeyEvent},
    layout::Rect,
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    Frame,
};
use rust_decimal::Decimal;
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::{
    color::ColorTheme,
    data::{
        Attribute, KeySchemaElement, KeySchemaType, KeyType, QueryRequest, ScalarAttributeType,
        SortKeyCondition, TableDescription,
    },
    event::{AppEvent, Sender, UserEvent, UserEventMapper},
    help::{
        build_help_spans, build_short_help_spans, BuildHelpsItem, BuildShortHelpsItem, Spans,
        SpansWithPriority,
    },
};

const SORT_OPS: [(SortOp, &str); 7] = [
    (SortOp::Eq, "="),
    (SortOp::BeginsWith, "begins_with"),
    (SortOp::Lt, "<"),
    (SortOp::Le, "<="),
    (SortOp::Gt, ">"),
    (SortOp::Ge, ">="),
    (SortOp::Between, "between"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortOp {
    Eq,
    BeginsWith,
    Lt,
    Le,
    Gt,
    Ge,
    Between,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Index,
    Pk,
    SortOp,
    Sk,
    Sk2,
}

struct IndexOption {
    label: String,
    schema: KeySchemaType,
}

pub struct QueryView {
    table_description: TableDescription,
    index_options: Vec<IndexOption>,
    index_idx: usize,
    pk_input: Input,
    sort_op_idx: usize,
    sk_input: Input,
    sk_input2: Input,
    focus: usize,
    error: Option<String>,

    helps: Vec<Spans>,
    helps_short: Vec<SpansWithPriority>,
    theme: ColorTheme,
    tx: Sender,
}

impl QueryView {
    pub fn new(
        table_description: TableDescription,
        mapper: &UserEventMapper,
        theme: ColorTheme,
        tx: Sender,
    ) -> Self {
        let mut index_options = vec![IndexOption {
            label: "(table)".to_string(),
            schema: table_description.key_schema_type.clone(),
        }];
        if let Some(gsis) = &table_description.global_secondary_indexes {
            for gsi in gsis {
                index_options.push(IndexOption {
                    label: gsi.index_name.clone(),
                    schema: schema_of(&gsi.key_schema),
                });
            }
        }
        if let Some(lsis) = &table_description.local_secondary_indexes {
            for lsi in lsis {
                index_options.push(IndexOption {
                    label: lsi.index_name.clone(),
                    schema: schema_of(&lsi.key_schema),
                });
            }
        }

        QueryView {
            table_description,
            index_options,
            index_idx: 0,
            pk_input: Input::default(),
            sort_op_idx: 0,
            sk_input: Input::default(),
            sk_input2: Input::default(),
            focus: 0,
            error: None,
            helps: build_helps(mapper, theme),
            helps_short: build_short_helps(mapper),
            theme,
            tx,
        }
    }

    fn current_schema(&self) -> &KeySchemaType {
        &self.index_options[self.index_idx].schema
    }

    fn has_sort_key(&self) -> bool {
        matches!(self.current_schema(), KeySchemaType::HashRange(_, _))
    }

    fn current_op(&self) -> SortOp {
        SORT_OPS[self.sort_op_idx].0
    }

    fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Index, Field::Pk];
        if self.has_sort_key() {
            fields.push(Field::SortOp);
            fields.push(Field::Sk);
            if self.current_op() == SortOp::Between {
                fields.push(Field::Sk2);
            }
        }
        fields
    }

    fn focused_field(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }
}

impl QueryView {
    pub fn handle_user_key_event(&mut self, user_events: Vec<UserEvent>, key_event: KeyEvent) {
        let has = |e: UserEvent| user_events.contains(&e);

        if has(UserEvent::Reset) {
            self.tx.send(AppEvent::BackToBeforeView);
            return;
        }
        if has(UserEvent::Confirm) {
            self.submit();
            return;
        }
        if has(UserEvent::NextPane) {
            let len = self.fields().len();
            self.focus = (self.focus + 1) % len;
            return;
        }

        match self.focused_field() {
            Field::Index => {
                if has(UserEvent::Right) {
                    self.index_idx = (self.index_idx + 1) % self.index_options.len();
                    self.clamp_focus();
                } else if has(UserEvent::Left) {
                    self.index_idx =
                        (self.index_idx + self.index_options.len() - 1) % self.index_options.len();
                    self.clamp_focus();
                }
            }
            Field::SortOp => {
                if has(UserEvent::Right) {
                    self.sort_op_idx = (self.sort_op_idx + 1) % SORT_OPS.len();
                    self.clamp_focus();
                } else if has(UserEvent::Left) {
                    self.sort_op_idx = (self.sort_op_idx + SORT_OPS.len() - 1) % SORT_OPS.len();
                    self.clamp_focus();
                }
            }
            Field::Pk => {
                self.pk_input.handle_event(&Event::Key(key_event));
            }
            Field::Sk => {
                self.sk_input.handle_event(&Event::Key(key_event));
            }
            Field::Sk2 => {
                self.sk_input2.handle_event(&Event::Key(key_event));
            }
        }
    }

    fn clamp_focus(&mut self) {
        let len = self.fields().len();
        if self.focus >= len {
            self.focus = len - 1;
        }
    }

    fn submit(&mut self) {
        let schema = self.current_schema().clone();
        let (pk_name, sk_name) = match &schema {
            KeySchemaType::Hash(pk) => (pk.clone(), None),
            KeySchemaType::HashRange(pk, sk) => (pk.clone(), Some(sk.clone())),
        };

        let pk_raw = self.pk_input.value().trim().to_string();
        if pk_raw.is_empty() {
            self.error = Some("partition key value is required".to_string());
            return;
        }
        let pk_value = match self.build_scalar(&pk_name, &pk_raw) {
            Ok(v) => v,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };

        let mut sort_key = None;
        if let Some(sk_name) = sk_name {
            let sk_raw = self.sk_input.value().trim().to_string();
            if !sk_raw.is_empty() {
                let cond = match self.current_op() {
                    SortOp::BeginsWith => SortKeyCondition::BeginsWith(sk_raw),
                    SortOp::Between => {
                        let sk2_raw = self.sk_input2.value().trim().to_string();
                        if sk2_raw.is_empty() {
                            self.error = Some("`between` needs a second value".to_string());
                            return;
                        }
                        match (
                            self.build_scalar(&sk_name, &sk_raw),
                            self.build_scalar(&sk_name, &sk2_raw),
                        ) {
                            (Ok(a), Ok(b)) => SortKeyCondition::Between(a, b),
                            (Err(e), _) | (_, Err(e)) => {
                                self.error = Some(e);
                                return;
                            }
                        }
                    }
                    op => {
                        let v = match self.build_scalar(&sk_name, &sk_raw) {
                            Ok(v) => v,
                            Err(e) => {
                                self.error = Some(e);
                                return;
                            }
                        };
                        match op {
                            SortOp::Eq => SortKeyCondition::Eq(v),
                            SortOp::Lt => SortKeyCondition::Lt(v),
                            SortOp::Le => SortKeyCondition::Le(v),
                            SortOp::Gt => SortKeyCondition::Gt(v),
                            SortOp::Ge => SortKeyCondition::Ge(v),
                            _ => unreachable!(),
                        }
                    }
                };
                sort_key = Some((sk_name, cond));
            }
        }

        let index_name = if self.index_idx == 0 {
            None
        } else {
            Some(self.index_options[self.index_idx].label.clone())
        };

        let request = QueryRequest {
            index_name,
            partition_key: (pk_name, pk_value),
            sort_key,
        };
        self.tx
            .send(AppEvent::RunQuery(self.table_description.clone(), request));
    }

    /// Build an S or N attribute for a key value based on the attribute's declared
    /// scalar type (defaults to String when unknown).
    fn build_scalar(&self, attr_name: &str, raw: &str) -> Result<Attribute, String> {
        let scalar = self
            .table_description
            .attribute_definitions
            .iter()
            .find(|d| d.attribute_name == attr_name)
            .map(|d| d.attribute_type);
        match scalar {
            Some(ScalarAttributeType::N) => Decimal::from_str(raw)
                .map(Attribute::N)
                .map_err(|_| format!("`{attr_name}` must be a number (got `{raw}`)")),
            _ => Ok(Attribute::S(raw.to_string())),
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let schema = self.current_schema().clone();
        let (pk_name, sk_name) = match &schema {
            KeySchemaType::Hash(pk) => (pk.clone(), None),
            KeySchemaType::HashRange(pk, sk) => (pk.clone(), Some(sk.clone())),
        };
        let focused = self.focused_field();

        let block = Block::bordered()
            .title_top(Line::from(format!(" Query {} ", self.table_description.table_name)).left_aligned())
            .fg(self.theme.fg)
            .bg(self.theme.bg)
            .padding(Padding::uniform(1));
        let inner = block.inner(area);

        let sel = |on: bool, s: String| -> Span<'static> {
            if on {
                s.fg(self.theme.selected_fg).bg(self.theme.selected_bg).bold()
            } else {
                s.fg(self.theme.fg)
            }
        };

        let mut lines: Vec<Line> = Vec::new();
        // index
        lines.push(Line::from(vec![
            "Index  ".bold(),
            sel(
                focused == Field::Index,
                format!("< {} >", self.index_options[self.index_idx].label),
            ),
        ]));
        // pk
        lines.push(Line::from(vec![
            format!("{pk_name}  = ").bold(),
            sel(focused == Field::Pk, format!("[ {} ]", self.pk_input.value())),
        ]));
        // sk
        if let Some(sk_name) = &sk_name {
            let mut sk_spans = vec![
                format!("{sk_name}  ").bold(),
                sel(focused == Field::SortOp, format!("< {} >", SORT_OPS[self.sort_op_idx].1)),
                "  ".into(),
            ];
            if self.current_op() != SortOp::BeginsWith
                || true /* value input applies to all ops */
            {
                sk_spans.push(sel(focused == Field::Sk, format!("[ {} ]", self.sk_input.value())));
            }
            lines.push(Line::from(sk_spans));
            if self.current_op() == SortOp::Between {
                lines.push(Line::from(vec![
                    "   and  ".bold(),
                    sel(focused == Field::Sk2, format!("[ {} ]", self.sk_input2.value())),
                ]));
            }
        }
        lines.push(Line::from(""));
        if let Some(err) = &self.error {
            lines.push(Line::from(err.as_str().fg(self.theme.notification_error).bold()));
        } else {
            lines.push(Line::from(
                "Tab: next field   ←/→: change   Enter: run   Esc: cancel"
                    .fg(self.theme.short_help),
            ));
        }

        f.render_widget(Paragraph::new(lines).block(block), area);

        // place the terminal cursor inside the focused text input
        let cursor_line = match focused {
            Field::Pk => Some((1u16, pk_name.len() as u16 + 4, self.pk_input.visual_cursor())),
            Field::Sk => sk_name
                .as_ref()
                .map(|n| (2u16, n.len() as u16 + 2 + SORT_OPS[self.sort_op_idx].1.len() as u16 + 4, self.sk_input.visual_cursor())),
            Field::Sk2 => Some((3u16, 8u16, self.sk_input2.visual_cursor())),
            _ => None,
        };
        if let Some((row, col, cur)) = cursor_line {
            let x = inner.x + col + cur as u16;
            let y = inner.y + row;
            f.set_cursor_position((x, y));
        }
    }

    pub fn short_helps(&self) -> &[SpansWithPriority] {
        &self.helps_short
    }
}

fn schema_of(key_schema: &[KeySchemaElement]) -> KeySchemaType {
    let mut hash = None;
    let mut range = None;
    for e in key_schema {
        match e.key_type {
            KeyType::Hash => hash = Some(e.attribute_name.clone()),
            KeyType::Range => range = Some(e.attribute_name.clone()),
        }
    }
    match (hash, range) {
        (Some(h), Some(r)) => KeySchemaType::HashRange(h, r),
        (Some(h), None) => KeySchemaType::Hash(h),
        _ => KeySchemaType::Hash(String::new()),
    }
}

fn build_helps(mapper: &UserEventMapper, theme: ColorTheme) -> Vec<Spans> {
    #[rustfmt::skip]
    let helps = vec![
        BuildHelpsItem::new(UserEvent::Quit, "Quit app"),
        BuildHelpsItem::new(UserEvent::Reset, "Cancel"),
        BuildHelpsItem::new(UserEvent::NextPane, "Next field"),
        BuildHelpsItem::new(UserEvent::Left, "Previous option"),
        BuildHelpsItem::new(UserEvent::Right, "Next option"),
        BuildHelpsItem::new(UserEvent::Confirm, "Run query"),
    ];
    build_help_spans(helps, mapper, theme)
}

fn build_short_helps(mapper: &UserEventMapper) -> Vec<SpansWithPriority> {
    #[rustfmt::skip]
    let helps = vec![
        BuildShortHelpsItem::single(UserEvent::Quit, "Quit", 0),
        BuildShortHelpsItem::single(UserEvent::Reset, "Cancel", 1),
        BuildShortHelpsItem::single(UserEvent::NextPane, "Next field", 2),
        BuildShortHelpsItem::group(vec![UserEvent::Left, UserEvent::Right], "Change", 3),
        BuildShortHelpsItem::single(UserEvent::Confirm, "Run", 0),
    ];
    build_short_help_spans(helps, mapper)
}
