//! Line-level parsing: UTF-8 validation, compound-message splitting and SCPI header-path
//! semantics (`docs/PLAN.md` 7.2).

use crate::command::{Command, ParseError};
use crate::lexer::lex_message;
use crate::tree::resolve;

/// Longest input line the server will accept. Anything longer is a `-100` and the remainder
/// is discarded up to the next terminator.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Split a compound message on `;`, honouring quoted strings.
///
/// # Errors
/// [`ParseError::Command`] if a quoted string is left unterminated.
pub fn split_messages(line: &str) -> Result<Vec<&str>, ParseError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;

    for (index, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                ';' => {
                    parts.push(&line[start..index]);
                    start = index + c.len_utf8();
                }
                _ => {}
            },
        }
    }

    if quote.is_some() {
        return Err(ParseError::command("unterminated quoted string"));
    }
    parts.push(&line[start..]);
    Ok(parts)
}

/// Parse one input line into the commands it contains.
///
/// Compound messages follow standard SCPI header-path semantics: after a message whose header
/// is `CONF:REC:DUR`, the current path is `CONF:REC`, so a following `DUR?` in the same line
/// resolves to `CONF:REC:DUR?`. A leading `:` or a `*` common command resets the path to the
/// root.
///
/// An empty line yields no commands and is not an error — some clients send a bare newline as
/// a keep-alive.
///
/// # Errors
/// The first [`ParseError`] encountered. A failure aborts the rest of the line, which is the
/// IEEE 488.2 behaviour for a compound message.
pub fn parse_line(line: &[u8]) -> Result<Vec<Command>, ParseError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ParseError::command(format!(
            "line of {} bytes exceeds the {MAX_LINE_BYTES} byte limit",
            line.len()
        )));
    }

    let text =
        std::str::from_utf8(line).map_err(|_| ParseError::command("input is not valid UTF-8"))?;
    let text = text.trim_end_matches(['\r', '\n']).trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let mut commands = Vec::new();
    let mut header_path: Vec<String> = Vec::new();

    for part in split_messages(text)? {
        if part.trim().is_empty() {
            return Err(ParseError::command("empty message in compound line"));
        }
        let mut message = lex_message(part)?;

        if message.is_common {
            // Common commands do not disturb the header path.
        } else if message.leading_colon || header_path.is_empty() {
            header_path = message.nodes.clone();
            header_path.pop();
        } else {
            let mut absolute = header_path.clone();
            absolute.extend(message.nodes.iter().cloned());
            message.nodes = absolute;
            header_path = message.nodes.clone();
            header_path.pop();
        }

        commands.push(resolve(&message)?);
    }

    Ok(commands)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::{ExportFormat, ScpiError};

    #[test]
    fn a_single_command_parses() {
        assert_eq!(parse_line(b"*IDN?").unwrap(), vec![Command::Idn]);
    }

    #[test]
    fn both_line_terminators_are_accepted() {
        assert_eq!(parse_line(b"*IDN?\n").unwrap(), vec![Command::Idn]);
        assert_eq!(parse_line(b"*IDN?\r\n").unwrap(), vec![Command::Idn]);
        assert_eq!(parse_line(b"  *IDN?  \r\n").unwrap(), vec![Command::Idn]);
    }

    #[test]
    fn an_empty_line_is_not_an_error() {
        assert!(parse_line(b"").unwrap().is_empty());
        assert!(parse_line(b"\r\n").unwrap().is_empty());
        assert!(parse_line(b"   ").unwrap().is_empty());
    }

    #[test]
    fn compound_messages_are_split() {
        assert_eq!(
            parse_line(b"*CLS;*IDN?").unwrap(),
            vec![Command::Cls, Command::Idn]
        );
    }

    #[test]
    fn header_path_carries_across_a_compound_message() {
        assert_eq!(
            parse_line(b"CONF:REC:DUR 2.5;DUR?").unwrap(),
            vec![
                Command::ConfigureDuration(2.5),
                Command::ConfigureDurationQuery
            ]
        );
    }

    #[test]
    fn a_leading_colon_resets_the_header_path() {
        assert_eq!(
            parse_line(b"CONF:REC:DUR 2.5;:FORM?").unwrap(),
            vec![Command::ConfigureDuration(2.5), Command::FormatQuery]
        );
    }

    #[test]
    fn common_commands_do_not_disturb_the_header_path() {
        assert_eq!(
            parse_line(b"CONF:REC:DUR 1;*CLS;DUR?").unwrap(),
            vec![
                Command::ConfigureDuration(1.0),
                Command::Cls,
                Command::ConfigureDurationQuery
            ]
        );
    }

    #[test]
    fn semicolons_inside_quotes_do_not_split() {
        assert_eq!(
            parse_line(br#"MMEM:LOAD:STAT "a;b.proj""#).unwrap(),
            vec![Command::MemoryLoadState("a;b.proj".into())]
        );
    }

    #[test]
    fn non_utf8_input_is_a_command_error() {
        let err = parse_line(&[0xFF, 0xFE, 0x00]).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::CommandError);
    }

    #[test]
    fn an_over_long_line_is_a_command_error() {
        let line = vec![b'A'; MAX_LINE_BYTES + 1];
        assert_eq!(
            parse_line(&line).unwrap_err().scpi_error(),
            ScpiError::CommandError
        );
    }

    #[test]
    fn a_line_at_exactly_the_limit_is_parsed() {
        let mut line = b"MMEM:LOAD:STAT \"".to_vec();
        line.resize(MAX_LINE_BYTES - 1, b'a');
        line.push(b'"');
        assert_eq!(line.len(), MAX_LINE_BYTES);
        assert!(parse_line(&line).is_ok());
    }

    #[test]
    fn an_empty_message_inside_a_compound_line_is_an_error() {
        assert_eq!(
            parse_line(b"*CLS;;*IDN?").unwrap_err().scpi_error(),
            ScpiError::CommandError
        );
    }

    #[test]
    fn a_failure_aborts_the_rest_of_the_line() {
        let err = parse_line(b"*CLS;NOPE?;*IDN?").unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::UndefinedHeader);
    }

    #[test]
    fn a_full_uts_sequence_parses() {
        let line = b"FORM TXT;:CONF:REC:DUR 0.1;:INIT";
        assert_eq!(
            parse_line(line).unwrap(),
            vec![
                Command::Format(ExportFormat::Txt),
                Command::ConfigureDuration(0.1),
                Command::Initiate,
            ]
        );
    }

    #[test]
    fn split_messages_reports_unterminated_quotes() {
        assert!(split_messages("MMEM:LOAD:STAT \"oops").is_err());
    }
}
