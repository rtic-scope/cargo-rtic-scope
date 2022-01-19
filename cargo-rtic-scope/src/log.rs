//! Auxilliary functions for logging information to `stdout`.
use colored::Colorize;
use crossterm::{
    cursor,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use std::io::stderr;

fn indent_with(header: colored::ColoredString, msg: String) {
    // clear current line
    let _ = stderr().execute(Clear(ClearType::CurrentLine));

    let _ = stderr().execute(cursor::MoveToColumn(0));
    eprint!("{:>12} ", header);
    for (i, line) in msg.lines().enumerate() {
        if i == 0 {
            eprintln!("{}", line);
        } else {
            eprintln!("{:>12} {}", " ", line);
        }
    }
}

pub fn cont_status(header: &str, msg: String) {
    let _ = stderr().execute(cursor::MoveToColumn(0));
    eprint!("{:>12} {}", header.green().bold(), msg);
    let _ = stderr().execute(cursor::MoveToColumn(0));
}

pub fn status(header: &str, msg: String) {
    indent_with(header.green().bold(), msg);
}

pub fn warn(msg: String) {
    indent_with("Warning".yellow().bold(), msg);
}

pub fn err(msg: String) {
    indent_with("Error".red().bold(), msg);
}

pub fn frontend(msg: String) {
    indent_with("Frontend".cyan().bold(), msg);
}

pub fn hint(msg: String) {
    indent_with("Hint".blue().bold(), msg);
}
