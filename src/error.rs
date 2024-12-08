use std::fmt;
use std::error::Error;

#[derive(Debug)]
pub enum PluginError {
    Io(std::io::Error),
    Plugin(String),
    Network(String),
    Hls(String),
    Storage(String),
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Io(e) => write!(f, "IO error: {}", e),
            PluginError::Plugin(s) => write!(f, "Plugin error: {}", s),
            PluginError::Network(s) => write!(f, "Network error: {}", s),
            PluginError::Hls(s) => write!(f, "HLS error: {}", s),
            PluginError::Storage(s) => write!(f, "Storage error: {}", s),
        }
    }
}

impl Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(err: std::io::Error) -> Self {
        PluginError::Io(err)
    }
} 