//! Everything the recorder does that is not the desktop shell: detecting
//! matches, driving ShadowPlay, naming recordings and keeping the folder
//! small. Each module's doc comment explains its part.
//!
//! Only `platform` touches the operating system directly. The rest builds and
//! is tested on any OS, with `engine` taking its OS access through `Host`.

pub mod activity;
pub mod engine;
pub mod game;
pub mod overlay;
pub mod overlay_setup;
pub mod platform;
pub mod recorder;
pub mod retention;
pub mod session;
pub mod settings;
pub mod setup;
pub mod steam;
pub mod tail;
pub mod test_recording;
pub mod update;
