mod agent;
mod tools;
use dotenvy::dotenv;
use futures::StreamExt;
use std::io::Write;
use std::{fmt, io};

use crate::agent::{Agent, AgentEvent};
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
    let agent = Agent::new();
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
                let mut response = agent.chat(value);
                while let Some(event) = response.next().await {
                    match event {
                        AgentEvent::TextChunk(text) => {
                            log!(Ansi::Cyan, "{}", text);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}
