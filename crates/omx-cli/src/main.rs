use std::io::{self, Write};
use std::process::ExitCode;

fn exit_code_from_i32(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(value) => ExitCode::from(value),
        Err(_) => ExitCode::from(1),
    }
}

fn main() -> ExitCode {
    match omx_cli::parse_args(std::env::args()) {
        omx_cli::CliAction::Help => {
            print!("{}", omx_cli::help_output());
            ExitCode::SUCCESS
        }
        omx_cli::CliAction::Version => {
            print!("{}", omx_cli::version_output());
            ExitCode::SUCCESS
        }
        omx_cli::CliAction::Ask(args) => match omx_cli::run_ask_command(&args) {
            Ok(result) => {
                io::stdout().write_all(&result.stdout).ok();
                io::stderr().write_all(&result.stderr).ok();
                exit_code_from_i32(result.exit_code)
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        omx_cli::CliAction::Reasoning(args) => {
            match omx_cli::reasoning::run_reasoning_command(&args, omx_cli::help_output()) {
                Ok(output) => {
                    print!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("Error: {error}");
                    ExitCode::from(1)
                }
            }
        }
        omx_cli::CliAction::Unsupported => {
            eprintln!("Unsupported command in Rust scaffold. Try: omx --help");
            ExitCode::from(1)
        }
    }
}
