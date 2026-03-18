use rust_embed::Embed;

mod add;
mod clean;
mod cmd;
mod metadata;
mod scaffold;

pub use cmd::AgentAction;

#[derive(Embed)]
#[folder = "assets/"]
pub struct Assets;
