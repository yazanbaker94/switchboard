#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod notifications;
mod permissions;
mod preferences;
mod shortcuts;
mod toast_activation;
mod tray_state;
mod workspace;

fn main() {
    workspace::run();
}
