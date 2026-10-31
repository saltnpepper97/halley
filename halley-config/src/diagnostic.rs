use std::fs;
use std::path::{Path, PathBuf};

use rune_cfg::RuneError;

use crate::RuntimeConfigError;

/// Terminal-friendly details for one failed configuration load.
///
/// Parser errors retain their source location and hint. Higher-level
/// validation errors often do not have a source span, so those fields remain
/// absent instead of pointing at a misleading line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    pub source_line: Option<String>,
    pub hint: Option<String>,
    pub code: Option<u32>,
}

impl ConfigDiagnostic {
    pub fn message(path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            path,
            line: None,
            column: None,
            message: message.into(),
            source_line: None,
            hint: None,
            code: None,
        }
    }

    pub fn from_runtime_error(path: &Path, error: &RuntimeConfigError) -> Self {
        let mut diagnostic = match error {
            RuntimeConfigError::Rune(error) => from_rune_error(error),
            RuntimeConfigError::Keybind(crate::ParseError::Rune(error))
            | RuntimeConfigError::Overlay(crate::OverlayParseError::Rune(error)) => {
                from_rune_error(error)
            }
            _ => Self::message(Some(path.to_path_buf()), error.to_string()),
        };
        diagnostic.path = Some(path.to_path_buf());
        diagnostic.source_line = diagnostic.line.and_then(|line| source_line(path, line));
        diagnostic
    }
}

fn from_rune_error(error: &RuneError) -> ConfigDiagnostic {
    let (message, line, column, hint, code) = match error {
        RuneError::SyntaxError {
            message,
            line,
            column,
            hint,
            code,
        }
        | RuneError::UnexpectedEof {
            message,
            line,
            column,
            hint,
            code,
        }
        | RuneError::TypeError {
            message,
            line,
            column,
            hint,
            code,
        }
        | RuneError::ValidationError {
            message,
            line,
            column,
            hint,
            code,
        } => (
            message.clone(),
            nonzero(*line),
            nonzero(*column),
            hint.clone(),
            *code,
        ),
        RuneError::InvalidToken {
            token,
            line,
            column,
            hint,
            code,
        } => (
            format!("Invalid token {token:?}"),
            nonzero(*line),
            nonzero(*column),
            hint.clone(),
            *code,
        ),
        RuneError::UnclosedString {
            quote,
            line,
            column,
            hint,
            code,
        } => (
            format!("Unclosed string starting with {quote:?}"),
            nonzero(*line),
            nonzero(*column),
            hint.clone(),
            *code,
        ),
        RuneError::UnexpectedCharacter {
            character,
            line,
            column,
            hint,
            code,
        } => (
            format!("Unexpected character {character:?}"),
            nonzero(*line),
            nonzero(*column),
            hint.clone(),
            *code,
        ),
        RuneError::FileError {
            message,
            hint,
            code,
            ..
        }
        | RuneError::RuntimeError {
            message,
            hint,
            code,
        } => (message.clone(), None, None, hint.clone(), *code),
    };

    ConfigDiagnostic {
        path: None,
        line,
        column,
        message,
        source_line: None,
        hint,
        code,
    }
}

fn nonzero(value: usize) -> Option<usize> {
    (value > 0).then_some(value)
}

fn source_line(path: &Path, line: usize) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .nth(line.saturating_sub(1))
        .map(str::to_string)
}
