//! InvisBrowser.
//!
//!   invisbrowser [address]                       open a window
//!   invisbrowser --screenshot <address> <png>    render one page without a window (see headless.rs)

mod app;
mod filedialog;
mod headless;
mod internal;
mod loader;
mod settings;
mod window;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().position(|a| a == "--screenshot") {
        Some(i) => headless::run(&args[i + 1..]),
        None => window::run(args.into_iter().find(|a| !a.starts_with("--"))),
    };
    std::process::exit(code);
}
