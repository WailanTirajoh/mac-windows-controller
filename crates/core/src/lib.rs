//! Inti Sambung. Crate ini tidak bergantung pada Tauri supaya seluruh logika
//! jaringan, pairing, transfer, dan video bisa dites dengan `cargo test`.

pub mod capture;
pub mod clipboard;
pub mod discovery;
pub mod identity;
pub mod input;
pub mod net;
pub mod node;
pub mod pairing;
pub mod protocol;
pub mod session;
pub mod transfer;
pub mod trust;
pub mod video;
