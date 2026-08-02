//! Error type shared by the cart parser and the Lua runtime.

use std::fmt;

/// Anything that can go wrong loading or running a cart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The cart text could not be parsed.
    Cart(String),
    /// Lua raised an error. The payload includes the traceback when Lua
    /// provided one.
    Lua(String),
}

impl Error {
    /// The human-readable message (with traceback for Lua errors).
    pub fn message(&self) -> &str {
        match self {
            Error::Cart(m) | Error::Lua(m) => m,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cart(m) => write!(f, "cart error: {m}"),
            Error::Lua(m) => write!(f, "lua error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<mlua::Error> for Error {
    fn from(e: mlua::Error) -> Self {
        Error::Lua(e.to_string())
    }
}
