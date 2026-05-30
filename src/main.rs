mod agent;
mod memory;
mod tools;
use dotenvy::dotenv;
use futures::StreamExt;
use std::fmt::format;
use std::io::Write;
use std::{fmt, io, result};

use crate::agent::{Agent, AgentEvent};
use crate::memory::Memory;
use crate::tools::get_tools;
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub enum Ansi {
    Reset,
    Bold,
    Dim,
    Blue,
    Cyan,
    Green,
    Yellow,
    Red,
}
impl fmt::Display for Ansi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Ansi::Reset => "\x1b[0m",
            Ansi::Bold => "\x1b[1m",
            Ansi::Dim => "\x1b[2m",
            Ansi::Blue => "\x1b[34m",
            Ansi::Cyan => "\x1b[36m",
            Ansi::Green => "\x1b[32m",
            Ansi::Yellow => "\x1b[33m",
            Ansi::Red => "\x1b[31m",
        };
        write!(f, "{}", code)
    }
}
#[macro_export]
macro_rules! log {
    ($color:expr, $($arg:tt)*) => {{
        println!("{}{}{}", $color, format_args!($($arg)*), Ansi::Reset);
    }};
}

#[macro_export]
macro_rules! log_separator {
    () => {{
        let width = 80;
        let line = "─".repeat(width);
        println!("{}{}{}", Ansi::Dim, line, Ansi::Reset)
    }};
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());

    let system_content = format!(
        "You are the assistant. Current working directory: {}. When useful, call the available tools to perform file and shell operations.",
        cwd
    );

    let agent = Agent::new(get_tools());
    let mut memory = Memory::new(system_content.as_str());
    log!(Ansi::Bold, "nano-code");
    loop {
        log_separator!();
        print!("{}{}❯{} ", Ansi::Bold, Ansi::Blue, Ansi::Reset);
        io::stdout().flush()?;

        let mut input = String::new();
        // ? is popping the error so that error is thrown and run is stopped
        let bytes = io::stdin().read_line(&mut input)?;
        if bytes == 0 {
            break;
        }
        let value = input.trim();
        match value {
            "/q" => {
                log!(Ansi::Cyan, "Thanks for using nano-code!");
                break;
            }
            _ => {
                let mut response = agent.chat(value, &mut memory);
                while let Some(event) = response.next().await {
                    match event {
                        AgentEvent::TextChunk(text) => {
                            log!(Ansi::Cyan, "{}", text);
                        }
                        AgentEvent::ToolChunk(name, args) => {
                            let args_values: Vec<String> = args
                                .as_object()
                                .unwrap()
                                .values()
                                .map(|v| v.to_string().chars().take(30).collect::<String>())
                                .collect();
                            let args_preview = args_values.join(",");
                            log!(Ansi::Green, "{}({})", name.to_uppercase(), args_preview);
                            match agent.call_tool(&name.to_string(), args) {
                                Ok(result) => {
                                    let result_lines: Vec<&str> = result.split("\n").collect();
                                    let preview = result_lines[0];

                                    let mut first_60 = preview.chars().take(60).collect::<String>();

                                    if result_lines.len() > 1 {
                                        first_60.push_str(&format!(
                                            "... + {} lines",
                                            result_lines.len() - 1
                                        ));
                                    }

                                    log!(Ansi::Dim, "⎿ {}", first_60);
                                }
                                Err(e) => {
                                    log!(Ansi::Red, "Error {}", e);
                                }
                            }
                        }
                        AgentEvent::Error(e) => {
                            log!(Ansi::Red, "Error {}", e);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
