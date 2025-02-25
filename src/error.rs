use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Io(std::io::Error),
    AddrParse(std::net::AddrParseError),
    Hyper(hyper::Error),
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(err) => write!(f, "IO error: {}", err),
            AppError::AddrParse(err) => write!(f, "Address parse error: {}", err),
            AppError::Hyper(err) => write!(f, "Hyper error: {}", err),
            AppError::Other(msg) => write!(f, "Error: {}", msg),
        }
    }
}

// Implement `std::error::Error` for `AppError`
impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err)
    }
}

impl From<std::net::AddrParseError> for AppError {
    fn from(err: std::net::AddrParseError) -> Self {
        AppError::AddrParse(err)
    }
}

impl From<hyper::Error> for AppError {
    fn from(err: hyper::Error) -> Self {
        AppError::Hyper(err)
    }
}