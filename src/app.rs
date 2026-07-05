use std::sync::Arc;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    prelude::Backend,
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Padding, Paragraph},
    Frame, Terminal,
};
use tokio::spawn;

use crate::{
    client::Client,
    color::ColorTheme,
    config::Config,
    data::{Item, QueryRequest, Table, TableDescription, TableInsight},
    error::{AppError, AppResult},
    event::{AppEvent, Receiver, Sender, UserEvent, UserEventMapper},
    handle_user_events,
    help::{prune_spans_to_fit_width, Spans},
    view::{View, ViewStack},
    widget::LoadingDialog,
};

enum Status {
    None,
    NotificationSuccess(String),
    NotificationWarning(String),
    NotificationError(String),
    Input(String, Option<u16>),
}

/// Connection parameters needed to build a client once a profile is chosen.
#[derive(Clone)]
pub struct ConnParams {
    pub region: Option<String>,
    pub endpoint_url: Option<String>,
    pub default_region: String,
}

pub struct App {
    view_stack: ViewStack,

    config: Config,
    theme: ColorTheme,
    mapper: UserEventMapper,

    status: Status,
    loading: bool,

    client: Option<Arc<Client>>,
    conn: ConnParams,
    tx: Sender,
}

impl App {
    pub fn new(
        config: Config,
        theme: ColorTheme,
        mapper: UserEventMapper,
        conn: ConnParams,
        profile: Option<String>,
        profiles: Vec<String>,
        tx: Sender,
    ) -> Self {
        // With a profile already chosen (via --profile), go straight to loading;
        // otherwise start on the profile picker.
        let (initial_view, loading) = match &profile {
            Some(_) => (View::of_init(theme, tx.clone()), true),
            None => (
                View::of_profile_list(profiles, &mapper, theme, tx.clone()),
                false,
            ),
        };
        App {
            view_stack: ViewStack::new(initial_view),
            config,
            theme,
            mapper,
            status: Status::None,
            loading,
            client: None,
            conn,
            tx,
        }
    }

    fn client(&self) -> Arc<Client> {
        self.client.clone().expect("client used before a profile was selected")
    }
}

impl App {
    pub fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        rx: Receiver,
    ) -> Result<(), B::Error> {
        loop {
            terminal.draw(|f| self.render(f))?;
            match rx.recv() {
                AppEvent::Key(key_event) => {
                    let user_events = self.mapper.find_events(key_event);

                    handle_user_events! { user_events =>
                        UserEvent::Quit => {
                            return Ok(());
                        }
                    }

                    if self.loading {
                        // Ignore key inputs while loading (except quit)
                        continue;
                    }

                    match self.status {
                        Status::None | Status::Input(_, _) => {
                            // do nothing
                        }
                        Status::NotificationSuccess(_) | Status::NotificationWarning(_) => {
                            // Clear message and pass key input as is
                            self.clear_status();
                        }
                        Status::NotificationError(_) => {
                            if matches!(self.view_stack.current_view(), View::Init(_)) {
                                return Ok(());
                            }
                            // Clear message and cancel key input
                            self.clear_status();
                            continue;
                        }
                    }

                    self.view_stack
                        .current_view_mut()
                        .handle_user_key_event(user_events, key_event);
                }
                AppEvent::Resize(w, h) => {
                    let _ = (w, h);
                }
                AppEvent::SelectProfile(profile) => {
                    self.select_profile(profile);
                }
                AppEvent::ClientReady(client) => {
                    self.client_ready(client);
                }
                AppEvent::Initialize => {
                    self.initialize();
                }
                AppEvent::CompleteInitialize(result) => {
                    self.complete_initialize(result);
                }
                AppEvent::LoadTableDescription(table_name) => {
                    self.load_table_description(table_name);
                }
                AppEvent::CompleteLoadTableDescription(result) => {
                    self.complete_load_table_description(result);
                }
                AppEvent::LoadTableItems(desc) => {
                    self.load_table_items(desc);
                }
                AppEvent::CompleteLoadTableItems(desc, result) => {
                    self.complete_load_table_items(desc, result);
                }
                AppEvent::OpenItem(desc, item) => {
                    self.open_item(desc, item);
                }
                AppEvent::OpenQueryForm(desc) => {
                    self.open_query_form(desc);
                }
                AppEvent::RunQuery(desc, request) => {
                    self.run_query(desc, request);
                }
                AppEvent::OpenEditor(desc, item) => {
                    self.open_editor(desc, item);
                }
                AppEvent::SaveItem(desc, item) => {
                    self.save_item(desc, item);
                }
                AppEvent::CompleteSaveItem(desc, result) => {
                    self.complete_save_item(desc, result);
                }
                AppEvent::DeleteItem(desc, item) => {
                    self.delete_item(desc, item);
                }
                AppEvent::CompleteDeleteItem(desc, result) => {
                    self.complete_delete_item(desc, result);
                }
                AppEvent::OpenTableInsight(insight) => {
                    self.open_table_insight(insight);
                }
                AppEvent::OpenHelp(helps) => {
                    self.open_help(helps);
                }
                AppEvent::BackToBeforeView => {
                    self.back_to_before_view();
                }
                AppEvent::CopyToClipboard(name, content) => {
                    self.copy_to_clipboard(name, content);
                }
                AppEvent::ClearStatus => {
                    self.clear_status();
                }
                AppEvent::UpdateStatusInput(msg, cursor_pos) => {
                    self.update_status_input(msg, cursor_pos);
                }
                AppEvent::NotifySuccess(msg) => {
                    self.notify_success(msg);
                }
                AppEvent::NotifyWarning(msg) => {
                    self.notify_warning(msg);
                }
                AppEvent::NotifyError(msg) => {
                    self.notify_error(msg);
                }
            }
        }
    }
}

impl App {
    fn render(&mut self, f: &mut Frame) {
        let [view_area, status_line_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());

        self.view_stack.current_view_mut().render(f, view_area);
        self.render_status_line(f, status_line_area);
        self.render_loading_dialog(f);
    }

    fn render_status_line(&self, f: &mut Frame, area: Rect) {
        let text: Line = match &self.status {
            Status::None => {
                let helps = self.view_stack.current_view().short_helps();
                let spans = prune_spans_to_fit_width(helps, area.width as usize - 2, ", "); // -2 for padding
                Line::from(spans).fg(self.theme.short_help)
            }
            Status::NotificationSuccess(msg) => Line::from(
                msg.as_str()
                    .add_modifier(Modifier::BOLD)
                    .fg(self.theme.notification_success),
            ),
            Status::NotificationWarning(msg) => Line::from(
                msg.as_str()
                    .add_modifier(Modifier::BOLD)
                    .fg(self.theme.notification_warning),
            ),
            Status::NotificationError(msg) => Line::from(
                format!("ERROR: {msg}")
                    .add_modifier(Modifier::BOLD)
                    .fg(self.theme.notification_error),
            ),
            Status::Input(msg, _) => Line::from(msg.as_str().fg(self.theme.fg)),
        };
        let paragraph = Paragraph::new(text).block(
            Block::default()
                .style(Style::default().bg(self.theme.bg))
                .padding(Padding::horizontal(1)),
        );
        f.render_widget(paragraph, area);

        if let Status::Input(_, Some(cursor_pos)) = &self.status {
            let (x, y) = (area.x + cursor_pos + 1, area.y + 1);
            f.set_cursor_position((x, y));
        }
    }

    fn render_loading_dialog(&self, f: &mut Frame) {
        if self.loading {
            let dialog = LoadingDialog::default().theme(self.theme);
            f.render_widget(dialog, f.area());
        }
    }
}

impl App {
    fn select_profile(&mut self, profile: String) {
        self.loading = true;
        let conn = self.conn.clone();
        let tx = self.tx.clone();
        spawn(async move {
            let client = Client::new(
                conn.region.clone(),
                conn.endpoint_url.clone(),
                Some(profile),
                conn.default_region.clone(),
            )
            .await;
            tx.send(AppEvent::ClientReady(Arc::new(client)));
        });
    }

    fn client_ready(&mut self, client: Arc<Client>) {
        self.client = Some(client);
        self.tx.send(AppEvent::Initialize);
    }

    fn initialize(&self) {
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client.list_all_tables().await;
            tx.send(AppEvent::CompleteInitialize(result));
        });
    }

    fn complete_initialize(&mut self, result: AppResult<Vec<Table>>) {
        match result {
            Ok(tables) => {
                if tables.is_empty() {
                    self.loading = false;
                    self.tx
                        .send(AppEvent::NotifyWarning(AppError::msg("No tables found.")));
                } else {
                    let view = View::of_table_list(
                        tables,
                        &self.mapper,
                        self.config.ui.table_list.clone(),
                        self.theme,
                        self.tx.clone(),
                    );
                    self.view_stack.pop();
                    self.view_stack.push(view);
                    // not update loading here
                }
            }
            Err(e) => {
                self.tx.send(AppEvent::NotifyError(e));
                self.loading = false;
            }
        }
    }

    fn load_table_description(&mut self, name: String) {
        self.loading = true;
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client.describe_table(&name).await;
            tx.send(AppEvent::CompleteLoadTableDescription(result));
        });
    }

    fn complete_load_table_description(&mut self, result: AppResult<TableDescription>) {
        match result {
            Ok(desc) => {
                if let View::TableList(view) = self.view_stack.current_view_mut() {
                    view.set_table_description(desc);
                }
            }
            Err(e) => {
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
        self.loading = false;
    }

    fn load_table_items(&mut self, desc: TableDescription) {
        self.loading = true;
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client
                .scan_items(
                    &desc.table_name,
                    &desc.key_schema_type,
                    crate::client::DEFAULT_SCAN_LIMIT,
                )
                .await;
            tx.send(AppEvent::CompleteLoadTableItems(desc, result));
        });
    }

    fn complete_load_table_items(&mut self, desc: TableDescription, result: AppResult<Vec<Item>>) {
        match result {
            Ok(items) => {
                if matches!(self.view_stack.current_view(), View::Table(_)) {
                    // when reloading in table view, pop current table view first
                    self.view_stack.pop();
                }
                if items.is_empty() {
                    let msg = format!("Table {} has no items", desc.table_name);
                    self.tx.send(AppEvent::NotifyWarning(AppError::msg(msg)));
                } else {
                    let view = View::of_table(
                        desc,
                        items,
                        &self.mapper,
                        self.config.ui.table.clone(),
                        self.theme,
                        self.tx.clone(),
                    );
                    self.view_stack.push(view);
                }
            }
            Err(e) => {
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
        self.loading = false;
    }

    fn open_item(&mut self, desc: TableDescription, item: Item) {
        let view = View::of_item(desc, item, &self.mapper, self.theme, self.tx.clone());
        self.view_stack.push(view);
    }

    fn open_query_form(&mut self, desc: TableDescription) {
        let view = View::of_query(desc, &self.mapper, self.theme, self.tx.clone());
        self.view_stack.push(view);
    }

    fn run_query(&mut self, desc: TableDescription, request: QueryRequest) {
        // leave the query form; results replace the underlying table view
        self.view_stack.pop();
        self.loading = true;
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client
                .query_items(&desc.table_name, &request, &desc.key_schema_type)
                .await;
            tx.send(AppEvent::CompleteLoadTableItems(desc, result));
        });
    }

    fn open_editor(&mut self, desc: TableDescription, item: Option<Item>) {
        let view = View::of_edit(desc, item, &self.mapper, self.theme, self.tx.clone());
        self.view_stack.push(view);
    }

    fn save_item(&mut self, desc: TableDescription, item: Item) {
        self.loading = true;
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client.put_item(&desc.table_name, &item).await;
            tx.send(AppEvent::CompleteSaveItem(desc, result));
        });
    }

    fn complete_save_item(&mut self, desc: TableDescription, result: AppResult<()>) {
        self.loading = false;
        match result {
            Ok(()) => {
                // leave the editor and refresh the table to reflect the change
                self.view_stack.pop();
                self.tx
                    .send(AppEvent::NotifySuccess("Item saved".to_string()));
                self.tx.send(AppEvent::LoadTableItems(desc));
            }
            Err(e) => {
                // stay in the editor so edits aren't lost
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
    }

    fn delete_item(&mut self, desc: TableDescription, item: Item) {
        self.loading = true;
        let client = self.client();
        let tx = self.tx.clone();
        spawn(async move {
            let result = client
                .delete_item(&desc.table_name, &item, &desc.key_schema_type)
                .await;
            tx.send(AppEvent::CompleteDeleteItem(desc, result));
        });
    }

    fn complete_delete_item(&mut self, desc: TableDescription, result: AppResult<()>) {
        self.loading = false;
        match result {
            Ok(()) => {
                self.tx
                    .send(AppEvent::NotifySuccess("Item deleted".to_string()));
                self.tx.send(AppEvent::LoadTableItems(desc));
            }
            Err(e) => {
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
    }

    fn open_table_insight(&mut self, insight: TableInsight) {
        let view = View::of_table_insight(insight, &self.mapper, self.theme, self.tx.clone());
        self.view_stack.push(view);
    }

    fn open_help(&mut self, helps: Vec<Spans>) {
        let view = View::of_help(helps, &self.mapper, self.theme, self.tx.clone());
        self.view_stack.push(view);
    }

    fn back_to_before_view(&mut self) {
        self.view_stack.pop();
    }

    fn copy_to_clipboard(&self, name: String, content: String) {
        match crate::util::copy_to_clipboard(&content) {
            Ok(_) => {
                let msg = format!("Copied {name} to clipboard successfully");
                self.tx.send(AppEvent::NotifySuccess(msg));
            }
            Err(e) => {
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
    }

    fn clear_status(&mut self) {
        self.status = Status::None;
    }

    fn update_status_input(&mut self, msg: String, cursor_pos: Option<u16>) {
        self.status = Status::Input(msg, cursor_pos);
    }

    fn notify_success(&mut self, msg: String) {
        self.status = Status::NotificationSuccess(msg);
    }

    fn notify_warning(&mut self, e: AppError) {
        self.status = Status::NotificationWarning(e.msg);
    }

    fn notify_error(&mut self, e: AppError) {
        self.status = Status::NotificationError(e.msg);
    }
}
