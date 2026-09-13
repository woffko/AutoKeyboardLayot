//! Bounded control messages for one fresh installer-helper process session.
//! Decoding consumes sequence numbers, not package/install approvals.
use crate::PackId;
use serde::Deserialize;
use std::collections::BTreeSet;

const MAX_REQUEST_BYTES: usize = 8192;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    CheckCatalog { local_file: Option<String> },
    Select { view: u64, ids: Vec<String> },
    ConfirmDownload { view: u64 },
    ConfirmInstall { view: u64 },
    Poll {},
    Cancel {},
    Close {},
}

impl Command {
    pub fn selected_ids(&self) -> Result<BTreeSet<PackId>, ProtocolError> {
        let Self::Select { ids, .. } = self else {
            return Err(ProtocolError::Invalid);
        };
        if ids.len() > 64 {
            return Err(ProtocolError::Limit);
        }
        let mut selected = BTreeSet::new();
        for id in ids {
            let id = PackId::parse(id).map_err(|_| ProtocolError::Invalid)?;
            if !selected.insert(id) {
                return Err(ProtocolError::Invalid);
            }
        }
        Ok(selected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format: u32,
    session: String,
    sequence: u64,
    command: Command,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Invalid,
    Limit,
    Session,
    Sequence,
    Exhausted,
}

pub struct RequestDecoder {
    session: String,
    next: u64,
}
impl RequestDecoder {
    /// Session ID comes from the fresh helper handshake, never from a request.
    /// It is a correlation ID, not a security credential against same-user code.
    pub fn new(session: String) -> Result<Self, ProtocolError> {
        if session.len() != 32
            || !session
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ProtocolError::Session);
        }
        Ok(Self { session, next: 1 })
    }
    pub fn accept(&mut self, bytes: &[u8]) -> Result<Command, ProtocolError> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(ProtocolError::Limit);
        }
        let request: Request = serde_json::from_slice(bytes).map_err(|_| ProtocolError::Invalid)?;
        if request.format != 1 {
            return Err(ProtocolError::Invalid);
        }
        if request.session != self.session {
            return Err(ProtocolError::Session);
        }
        if request.sequence != self.next {
            return Err(ProtocolError::Sequence);
        }
        if matches!(request.command, Command::Select { .. }) {
            request.command.selected_ids()?;
        }
        self.next = self.next.checked_add(1).ok_or(ProtocolError::Exhausted)?;
        Ok(request.command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    fn request(sequence: u64, command: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"format":1,"session":SESSION,"sequence":sequence,"command":command})).unwrap()
    }
    #[test]
    fn only_sequenced_current_session_commands_are_consumed() {
        let mut decoder = RequestDecoder::new(SESSION.into()).unwrap();
        let poll = request(1, serde_json::json!({"action":"poll"}));
        let mut wrong: serde_json::Value = serde_json::from_slice(&poll).unwrap();
        wrong["session"] = serde_json::json!("00000000000000000000000000000000");
        assert_eq!(
            decoder
                .accept(&serde_json::to_vec(&wrong).unwrap())
                .unwrap_err(),
            ProtocolError::Session
        );
        assert!(matches!(decoder.accept(&poll).unwrap(), Command::Poll {}));
        assert_eq!(decoder.accept(&poll).unwrap_err(), ProtocolError::Sequence);
        assert_eq!(
            decoder
                .accept(&request(3, serde_json::json!({"action":"poll"})))
                .unwrap_err(),
            ProtocolError::Sequence
        );
        assert!(
            decoder
                .accept(&request(2, serde_json::json!({"action":"cancel"})))
                .is_ok()
        );
    }
    #[test]
    fn hostile_fields_duplicates_and_limits_never_become_commands() {
        let mut decoder = RequestDecoder::new(SESSION.into()).unwrap();
        for command in [
            serde_json::json!({"action":"poll","extra":true}),
            serde_json::json!({"action":"execute","path":"program.exe"}),
            serde_json::json!({"action":"select","view":1,"ids":["ru-RU","RU-ru"]}),
            serde_json::json!({"action":"select","view":1,"ids":vec!["ru-RU";65]}),
            serde_json::json!({"action":"confirm_install"}),
        ] {
            assert!(decoder.accept(&request(1, command)).is_err());
        }
        let duplicate = format!(
            r#"{{"format":1,"format":1,"session":"{SESSION}","sequence":1,"command":{{"action":"poll"}}}}"#
        );
        assert!(decoder.accept(duplicate.as_bytes()).is_err());
        assert_eq!(
            decoder.accept(&vec![b' '; 8193]).unwrap_err(),
            ProtocolError::Limit
        );
        let command = decoder
            .accept(&request(
                1,
                serde_json::json!({"action":"select","view":1,"ids":["ru-RU"]}),
            ))
            .unwrap();
        assert_eq!(
            command.selected_ids().unwrap(),
            BTreeSet::from([PackId::parse("ru-RU").unwrap()])
        );
    }
}
