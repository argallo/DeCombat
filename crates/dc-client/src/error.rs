//! Error type of the client slice.

use std::fmt;

#[derive(Debug)]
pub enum ClientError {
    /// The ROM folder or one of its archives could not be read.
    Rom(String),
    /// The requested world is not in the ROM.
    NoSuchMap(String),
    /// The world file did not parse.
    World(String),
    /// No usable spawn point.
    Spawn(String),
    /// Graphics device, surface or read-back failure.
    Gpu(String),
    /// Window system failure.
    Window(String),
    /// Bad command line.
    Usage(String),
    /// File output.
    Io(String),
    /// Lobby or game-server session.
    Net(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Rom(s) => write!(f, "ROM: {s}"),
            ClientError::NoSuchMap(s) => write!(f, "no world named {s} in the ROM"),
            ClientError::World(s) => write!(f, "world: {s}"),
            ClientError::Spawn(s) => write!(f, "spawn: {s}"),
            ClientError::Gpu(s) => write!(f, "graphics: {s}"),
            ClientError::Window(s) => write!(f, "window: {s}"),
            ClientError::Usage(s) => write!(f, "{s}"),
            ClientError::Io(s) => write!(f, "output: {s}"),
            ClientError::Net(s) => write!(f, "network: {s}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<dc_render::RenderError> for ClientError {
    fn from(e: dc_render::RenderError) -> Self {
        ClientError::Gpu(e.to_string())
    }
}
