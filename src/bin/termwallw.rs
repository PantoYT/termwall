//! termwallw: termwall without a console window, for autostart at logon (what pythonw is to
//! python). Same program, same arguments; nothing it prints is seen.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    termwall::run();
}
