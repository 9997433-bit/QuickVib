//! Turning one SCPI message into a header path and a list of arguments.
//!
//! A *message* is one element of a semicolon-separated compound line. Splitting the line is
//! [`crate::session::split_messages`]'s job; this module takes it from there.

use crate::command::ParseError;

/// One argument, before it is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Argument {
    /// A quoted string, with the quotes removed and doubled quotes unescaped.
    Quoted(String),
    /// A bare token: a number, a keyword such as `CSV`, or anything else unquoted.
    Token(String),
}

impl Argument {
    /// The argument's text, regardless of how it was written.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Quoted(s) | Self::Token(s) => s,
        }
    }
}

/// A lexed message: the header path, whether it is a query, and the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexedMessage {
    /// True when the header began with `:`, which resets the header path to the root.
    pub leading_colon: bool,
    /// True for `*`-prefixed IEEE 488.2 common commands.
    pub is_common: bool,
    /// The mnemonics of the header, in order, uppercased.
    pub nodes: Vec<String>,
    /// True when the header ended with `?`.
    pub query: bool,
    /// The arguments after the header.
    pub arguments: Vec<Argument>,
}

/// Lex one message.
///
/// # Errors
/// [`ParseError::Command`] for anything structurally malformed: an empty header, a mnemonic
/// with a non-alphanumeric character, an unterminated quoted string, a `?` in the middle of
/// the header, or an empty argument between two commas.
pub fn lex_message(message: &str) -> Result<LexedMessage, ParseError> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(ParseError::command("empty message"));
    }
    if !trimmed.is_ascii() {
        return Err(ParseError::command("message contains non-ASCII characters"));
    }

    // The header runs up to the first whitespace outside a quoted string.
    let split_at = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (header, rest) = trimmed.split_at(split_at);
    let arguments = lex_arguments(rest.trim())?;

    let mut header = header;
    let is_common = header.starts_with('*');
    if is_common {
        header = &header[1..];
    }

    let leading_colon = !is_common && header.starts_with(':');
    if leading_colon {
        header = &header[1..];
    }

    let query = header.ends_with('?');
    if query {
        header = &header[..header.len() - 1];
    }

    if header.is_empty() {
        return Err(ParseError::command("message has no header"));
    }

    let mut nodes = Vec::new();
    for node in header.split(':') {
        if node.is_empty() {
            return Err(ParseError::command(format!(
                "empty header node in '{trimmed}'"
            )));
        }
        if !node.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(ParseError::command(format!("illegal header node '{node}'")));
        }
        nodes.push(node.to_ascii_uppercase());
    }

    if is_common && nodes.len() != 1 {
        return Err(ParseError::command(
            "common commands have a single mnemonic",
        ));
    }

    Ok(LexedMessage {
        leading_colon,
        is_common,
        nodes,
        query,
        arguments,
    })
}

/// Split the argument list on commas, honouring quoted strings.
fn lex_arguments(text: &str) -> Result<Vec<Argument>, ParseError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut quoted_argument = false;
    // Set while sitting between a closing quote and the comma that ends the argument: only
    // whitespace may follow, so `"a"x` and `"a" "b"` are malformed rather than concatenated.
    let mut after_quote = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => {
                // A doubled quote inside a quoted string is a literal quote.
                if chars.peek() == Some(&q) {
                    current.push(q);
                    chars.next();
                } else {
                    quote = None;
                    after_quote = true;
                }
            }
            Some(_) => current.push(c),
            None if after_quote => match c {
                ',' => {
                    arguments.push(finish_argument(&current, quoted_argument)?);
                    current.clear();
                    quoted_argument = false;
                    after_quote = false;
                }
                _ if c.is_whitespace() => {}
                _ => {
                    return Err(ParseError::command(
                        "trailing characters after a quoted string",
                    ))
                }
            },
            None => match c {
                '"' | '\'' => {
                    if !current.trim().is_empty() {
                        return Err(ParseError::command(
                            "a quoted string must be the whole argument",
                        ));
                    }
                    current.clear();
                    quote = Some(c);
                    quoted_argument = true;
                }
                ',' => {
                    arguments.push(finish_argument(&current, quoted_argument)?);
                    current.clear();
                    quoted_argument = false;
                }
                _ => current.push(c),
            },
        }
    }

    if quote.is_some() {
        return Err(ParseError::command("unterminated quoted string"));
    }
    arguments.push(finish_argument(&current, quoted_argument)?);
    Ok(arguments)
}

fn finish_argument(text: &str, quoted: bool) -> Result<Argument, ParseError> {
    if quoted {
        return Ok(Argument::Quoted(text.to_owned()));
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ParseError::command("empty argument"));
    }
    Ok(Argument::Token(trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn nodes(message: &str) -> Vec<String> {
        lex_message(message).unwrap().nodes
    }

    #[test]
    fn header_is_uppercased_and_split_on_colons() {
        assert_eq!(nodes("conf:rec:dur 5"), vec!["CONF", "REC", "DUR"]);
        assert_eq!(
            nodes("CONFigure:RECord:DURation 5"),
            vec!["CONFIGURE", "RECORD", "DURATION"]
        );
    }

    #[test]
    fn common_commands_are_flagged() {
        let m = lex_message("*IDN?").unwrap();
        assert!(m.is_common);
        assert!(m.query);
        assert_eq!(m.nodes, vec!["IDN"]);
    }

    #[test]
    fn leading_colon_is_recorded_and_stripped() {
        let m = lex_message(":CONF:REC:DUR 1").unwrap();
        assert!(m.leading_colon);
        assert_eq!(m.nodes, vec!["CONF", "REC", "DUR"]);
    }

    #[test]
    fn query_suffix_is_detected() {
        assert!(lex_message("REC:STAT?").unwrap().query);
        assert!(!lex_message("REC:STAR").unwrap().query);
    }

    #[test]
    fn quoted_arguments_keep_spaces() {
        let m = lex_message(r#"MMEM:LOAD:STAT "C:\Test Files\Test.proj""#).unwrap();
        assert_eq!(
            m.arguments,
            vec![Argument::Quoted(r"C:\Test Files\Test.proj".to_owned())]
        );
    }

    #[test]
    fn doubled_quotes_are_unescaped() {
        let m = lex_message(r#"MMEM:STOR:TRAC "say ""hi"".csv""#).unwrap();
        assert_eq!(m.arguments[0].text(), r#"say "hi".csv"#);
    }

    #[test]
    fn single_quotes_are_accepted() {
        let m = lex_message("MMEM:LOAD:STAT 'Test.proj'").unwrap();
        assert_eq!(m.arguments[0], Argument::Quoted("Test.proj".to_owned()));
    }

    #[test]
    fn bare_tokens_are_trimmed() {
        let m = lex_message("FORM   CSV  ").unwrap();
        assert_eq!(m.arguments, vec![Argument::Token("CSV".to_owned())]);
    }

    #[test]
    fn multiple_arguments_split_on_commas() {
        let m = lex_message("FORM CSV, 4").unwrap();
        assert_eq!(
            m.arguments,
            vec![
                Argument::Token("CSV".to_owned()),
                Argument::Token("4".to_owned())
            ]
        );
    }

    #[test]
    fn commas_inside_quotes_do_not_split() {
        let m = lex_message(r#"MMEM:STOR:TRAC "a,b.csv""#).unwrap();
        assert_eq!(m.arguments.len(), 1);
        assert_eq!(m.arguments[0].text(), "a,b.csv");
    }

    #[test]
    fn no_arguments_yields_an_empty_list() {
        assert!(lex_message("INIT").unwrap().arguments.is_empty());
    }

    #[test]
    fn malformed_input_is_a_command_error() {
        for message in [
            "",
            "   ",
            "?",
            ":",
            "CONF::DUR 1",
            "CONF:REC:DUR? 1,",
            r#"MMEM:LOAD:STAT "unterminated"#,
            "*IDN:EXTRA?",
            "FORM CS-V\u{00e9}",
        ] {
            assert!(
                lex_message(message).is_err(),
                "expected failure for {message:?}"
            );
        }
    }

    #[test]
    fn characters_after_a_closing_quote_are_rejected() {
        for message in [
            r#"MMEM:LOAD:STAT "a"x"#,
            r#"MMEM:LOAD:STAT "a" "b""#,
            r#"MMEM:LOAD:STAT "a"'b'"#,
            r#"MMEM:LOAD:STAT x"a""#,
        ] {
            let err = lex_message(message).unwrap_err();
            assert_eq!(
                err.scpi_error(),
                quickvib_core::ScpiError::CommandError,
                "{message:?}"
            );
        }
    }

    #[test]
    fn whitespace_and_commas_may_follow_a_closing_quote() {
        let m = lex_message(r#"MMEM:STOR:TRAC "a.csv" , "b.csv"  "#).unwrap();
        assert_eq!(
            m.arguments,
            vec![
                Argument::Quoted("a.csv".to_owned()),
                Argument::Quoted("b.csv".to_owned()),
            ]
        );
    }

    #[test]
    fn illegal_header_characters_are_rejected() {
        let err = lex_message("CONF:RE-C:DUR 1").unwrap_err();
        assert_eq!(err.scpi_error(), quickvib_core::ScpiError::CommandError);
    }
}
