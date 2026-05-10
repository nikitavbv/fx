// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::path::PathBuf,
    tracing::{Level, info, warn},
    tracing_subscriber::FmtSubscriber,
    clap::{Parser, Subcommand},
    fx_runtime::{FxServer, config::ServerConfig},
};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(long)]
    debug: Option<bool>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        config_file: String,
    },
}

#[cfg(not(target_arch = "aarch64"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let args = Args::parse();
    FmtSubscriber::builder()
        .with_max_level(if args.debug.unwrap_or(false) { Level::DEBUG } else { Level::INFO })
        .init();

    match args.command {
        Command::Serve { config_file } => {
            let current_dir = match std::env::current_dir() {
                Ok(v) => v,
                Err(err) => {
                    let default_work_dir = PathBuf::from("/var/lib/fx");
                    warn!("failed to get workdir: {err:?}. Defaulting to {default_work_dir:?}");
                    default_work_dir
                }
            };
            let config_path = current_dir.join(config_file);

            info!("Loading config from {config_path:?}");
            let config = ServerConfig::load(config_path);

            FxServer::new(config).start().wait_until_finished();
        },
    }
}
