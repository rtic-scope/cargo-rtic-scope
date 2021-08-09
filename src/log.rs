use colored::Colorize;
use std::io::Write;

fn indent_with(header: colored::ColoredString, msg: String) {
    // erase already printed line
    std::io::stderr().write_all(&[27]).unwrap();
    eprint!("[2K\r");

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
    eprint!("\r{:>12} {}", header.green().bold(), msg);
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

pub fn hint(msg: String) {
    indent_with("Hint".blue().bold(), msg);
}
