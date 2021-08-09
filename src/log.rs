use colored::Colorize;

fn indent_with(header: colored::ColoredString, msg: String) {
    eprint!("{:>12} ", header);
    for (i, line) in msg.lines().enumerate() {
        if i == 0 {
            eprintln!("{}", line);
        } else {
            eprintln!("{:>12} ", line);
        }
    }
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
