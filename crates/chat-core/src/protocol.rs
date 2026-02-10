use serde::{Deserialize, Serialize};

pub const MAX_LINE: usize = 1024;
pub const MAX_NICK: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    Nick { nick: String },
    Say { text: String },
    Who,
    Quit,
    Prompt { id: String, answer: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    Sys { text: String },
    Msg { nick: String, text: String },
    Hist { nick: String, text: String },
    Who { count: usize, nicks: Vec<String> },
    Prompt { id: String, text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn clean_line(line: &str) -> Option<String> {
    let mut s = line.trim_end_matches(['\r', '\n']).to_string();
    if s.len() > MAX_LINE {
        s.truncate(MAX_LINE);
    }
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub fn parse_client_line(line: &str) -> Result<ClientMsg, ParseError> {
    let Some(clean) = clean_line(line) else {
        return Err(ParseError::new("empty line"));
    };
    let mut parts = clean.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match cmd.to_uppercase().as_str() {
        "NICK" => {
            let nick = rest;
            if nick.is_empty() {
                return Err(ParseError::new("missing nickname"));
            }
            Ok(ClientMsg::Nick {
                nick: nick.to_string(),
            })
        }
        "SAY" => {
            let text = rest;
            if text.is_empty() {
                return Err(ParseError::new("empty message"));
            }
            Ok(ClientMsg::Say {
                text: text.to_string(),
            })
        }
        "WHO" => Ok(ClientMsg::Who),
        "QUIT" => Ok(ClientMsg::Quit),
        "PROMPT" => {
            let mut parts = rest.splitn(2, ' ');
            let id = parts.next().unwrap_or("").trim().to_string();
            let answer = parts.next().unwrap_or("").trim().to_string();
            if id.is_empty() || answer.is_empty() {
                return Err(ParseError::new("invalid prompt reply"));
            }
            Ok(ClientMsg::Prompt { id, answer })
        }
        _ => Err(ParseError::new("unknown command")),
    }
}

pub fn format_server_msg(msg: &ServerMsg) -> String {
    match msg {
        ServerMsg::Sys { text } => format!("SYS {}", text),
        ServerMsg::Msg { nick, text } => format!("MSG {} {}", nick, text),
        ServerMsg::Hist { nick, text } => format!("HIST {} {}", nick, text),
        ServerMsg::Who { count, nicks } => {
            let list = nicks.join(" ");
            format!("WHO {} {}", count, list)
        }
        ServerMsg::Prompt { id, text } => format!("PROMPT {} {}", id, text),
    }
}

pub fn parse_server_line(line: &str) -> Result<ServerMsg, ParseError> {
    let Some(clean) = clean_line(line) else {
        return Err(ParseError::new("empty line"));
    };
    let mut parts = clean.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    match cmd.to_uppercase().as_str() {
        "SYS" => Ok(ServerMsg::Sys {
            text: rest.to_string(),
        }),
        "MSG" => {
            let mut parts = rest.splitn(2, ' ');
            let nick = parts.next().unwrap_or("").to_string();
            let text = parts.next().unwrap_or("").to_string();
            if nick.is_empty() || text.is_empty() {
                return Err(ParseError::new("invalid MSG"));
            }
            Ok(ServerMsg::Msg { nick, text })
        }
        "HIST" => {
            let mut parts = rest.splitn(2, ' ');
            let nick = parts.next().unwrap_or("").to_string();
            let text = parts.next().unwrap_or("").to_string();
            if nick.is_empty() || text.is_empty() {
                return Err(ParseError::new("invalid HIST"));
            }
            Ok(ServerMsg::Hist { nick, text })
        }
        "WHO" => {
            let mut parts = rest.splitn(2, ' ');
            let count_str = parts.next().unwrap_or("0");
            let count = count_str.parse::<usize>().unwrap_or(0);
            let nicks = parts
                .next()
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            Ok(ServerMsg::Who { count, nicks })
        }
        "PROMPT" => {
            let mut parts = rest.splitn(2, ' ');
            let id = parts.next().unwrap_or("").to_string();
            let text = parts.next().unwrap_or("").to_string();
            if id.is_empty() || text.is_empty() {
                return Err(ParseError::new("invalid PROMPT"));
            }
            Ok(ServerMsg::Prompt { id, text })
        }
        _ => Err(ParseError::new("unknown command")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_client_nick() {
        let msg = parse_client_line("NICK alice\n").unwrap();
        assert_eq!(msg, ClientMsg::Nick { nick: "alice".into() });
    }

    #[test]
    fn parse_client_say() {
        let msg = parse_client_line("SAY hello").unwrap();
        assert_eq!(msg, ClientMsg::Say { text: "hello".into() });
    }

    #[test]
    fn format_server_msg_line() {
        let line = format_server_msg(&ServerMsg::Sys { text: "hi".into() });
        assert_eq!(line, "SYS hi");
    }
}
