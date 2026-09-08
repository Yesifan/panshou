use std::process::ExitCode;

use clap::Parser;
use pansou::cli::{Cli, error_exit_code};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match pansou::cli::run(cli).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(error_exit_code(&error) as u8)
        }
    }
}
