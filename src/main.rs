use std::process::ExitCode;

use clap::Parser;
use pansou::cli::{Cli, error_exit_code, is_root_help_request};

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args_os().collect::<Vec<_>>();
    let root_help = is_root_help_request(&args[1..]);
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            if root_help {
                println!("\n{}", pansou::cli::help_defaults());
            }
            return ExitCode::from(code as u8);
        }
    };
    match pansou::cli::run(cli).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(error_exit_code(&error) as u8)
        }
    }
}
