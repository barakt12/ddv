mod app;
mod aws_profiles;
mod client;
mod color;
mod config;
mod constant;
mod data;
mod error;
mod event;
mod help;
mod item_json;
mod macros;
mod util;
mod view;
mod widget;

use clap::Parser;

use crate::{
    app::{App, ConnParams},
    color::ColorTheme,
    config::Config,
    event::UserEventMapper,
};

/// DDV - Terminal DynamoDB Viewer ⚡️
#[derive(Parser)]
#[command(version)]
struct Args {
    /// AWS region
    #[arg(short, long)]
    region: Option<String>,

    /// AWS endpoint url
    #[arg(short, long, value_name = "URL")]
    endpoint_url: Option<String>,

    /// AWS profile name
    #[arg(short, long, value_name = "NAME")]
    profile: Option<String>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let config = Config::load();
    let theme = ColorTheme::default();
    let mapper = UserEventMapper::new();

    let (tx, rx) = event::init();

    let conn = ConnParams {
        region: args.region,
        endpoint_url: args.endpoint_url,
        default_region: config.default_region.clone(),
    };
    let profile = args.profile;
    // Curated list from config if provided, else every profile in the AWS files.
    let profiles = if config.profiles.is_empty() {
        aws_profiles::list_profiles()
    } else {
        config.profiles.clone()
    };

    // If a profile was passed on the CLI, use it directly; otherwise the app
    // opens on a profile picker.
    if let Some(p) = &profile {
        tx.send(event::AppEvent::SelectProfile(p.clone()));
    }

    let mut terminal = ratatui::init();

    let mut app = App::new(config, theme, mapper, conn, profile, profiles, tx);
    let ret = app.run(&mut terminal, rx);

    ratatui::restore();
    ret
}
