//! SCPI parsing and response formatting.
//!
//! This crate is *pure*: it turns a line of bytes into [`Command`] values and a [`Response`]
//! into bytes. It knows nothing about the instrument, which is what keeps the parser trivially
//! unit-testable and the dependency graph acyclic (`docs/PLAN.md` 6.2, D6).
//!
//! ```
//! use quickvib_scpi::{parse_line, Command};
//!
//! assert_eq!(parse_line(b"*IDN?").unwrap(), vec![Command::Idn]);
//! // Short and long forms are interchangeable, and matching is ASCII case-insensitive.
//! assert_eq!(
//!     parse_line(b"configure:record:duration 5.0").unwrap(),
//!     vec![Command::ConfigureDuration(5.0)]
//! );
//! // Compound messages keep the SCPI header path across `;`.
//! assert_eq!(
//!     parse_line(b"CONF:REC:DUR 2.5;DUR?").unwrap(),
//!     vec![Command::ConfigureDuration(2.5), Command::ConfigureDurationQuery]
//! );
//! ```

#![forbid(unsafe_code)]

pub mod command;
pub mod format;
pub mod lexer;
pub mod session;
pub mod tree;

pub use command::{Command, ParseError};
pub use format::{write_response_line, Response, NOTIFY_RECORD_ABORTED, NOTIFY_RECORD_DONE};
pub use lexer::{lex_message, Argument, LexedMessage};
pub use session::{parse_line, split_messages, MAX_LINE_BYTES};
