#![cfg_attr(windows, windows_subsystem = "windows")]

mod icon_logic;
mod net_logic;
mod update_logic;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!(
        "macRust is a Windows 10/11 shell. To look at the UI on this system, run:\n  cargo run --release -p macrust-ui --example preview -- out"
    );
}
