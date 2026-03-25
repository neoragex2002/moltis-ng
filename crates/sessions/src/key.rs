#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSessionKey {
    Agent {
        agent_id: String,
        bucket_key: String,
    },
    System {
        service_id: String,
        bucket_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKeyParseError {
    InvalidShape,
    UnsupportedSystemService,
}

impl std::fmt::Display for SessionKeyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidShape => write!(f, "invalid session_key shape"),
            Self::UnsupportedSystemService => write!(f, "unsupported system service_id"),
        }
    }
}

impl std::error::Error for SessionKeyParseError {}

impl SessionKey {
    pub fn main(agent_id: &str) -> Self {
        Self::agent(agent_id, "main")
    }

    pub fn agent(agent_id: &str, bucket_key: &str) -> Self {
        Self(format!("agent:{agent_id}:{bucket_key}"))
    }

    pub fn system(service_id: &str, bucket_key: &str) -> Self {
        Self(format!("system:{service_id}:{bucket_key}"))
    }

    pub fn for_peer(
        agent_id: &str,
        _channel: &str,
        account: &str,
        peer_kind: &str,
        peer_id: &str,
    ) -> Self {
        Self::agent(
            agent_id,
            &format!("dm-peer-{peer_kind}.{peer_id}-account-{account}"),
        )
    }

    pub fn parse(value: &str) -> Result<ParsedSessionKey, SessionKeyParseError> {
        let mut parts = value.splitn(3, ':');
        let owner = parts.next().ok_or(SessionKeyParseError::InvalidShape)?;
        let subject = parts.next().ok_or(SessionKeyParseError::InvalidShape)?;
        let bucket_key = parts.next().ok_or(SessionKeyParseError::InvalidShape)?;
        if owner.is_empty() || subject.is_empty() || bucket_key.is_empty() {
            return Err(SessionKeyParseError::InvalidShape);
        }

        match owner {
            "agent" => Ok(ParsedSessionKey::Agent {
                agent_id: subject.to_string(),
                bucket_key: bucket_key.to_string(),
            }),
            "system" => {
                if subject != "cron" {
                    return Err(SessionKeyParseError::UnsupportedSystemService);
                }
                Ok(ParsedSessionKey::System {
                    service_id: subject.to_string(),
                    bucket_key: bucket_key.to_string(),
                })
            },
            _ => Err(SessionKeyParseError::InvalidShape),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedSessionKey, SessionKey};

    #[test]
    fn main_session_key_uses_canonical_agent_prefix() {
        assert_eq!(SessionKey::main("zhuzhu").0, "agent:zhuzhu:main");
    }

    #[test]
    fn peer_session_key_uses_canonical_bucket_grammar() {
        assert_eq!(
            SessionKey::for_peer(
                "zhuzhu",
                "telegram",
                "tguser.8344017527",
                "person",
                "neoragex2002"
            )
            .0,
            "agent:zhuzhu:dm-peer-person.neoragex2002-account-tguser.8344017527"
        );
    }

    #[test]
    fn parse_accepts_agent_session_key() {
        assert_eq!(
            SessionKey::parse("agent:zhuzhu:dm-peer-person.neoragex2002").unwrap(),
            ParsedSessionKey::Agent {
                agent_id: "zhuzhu".into(),
                bucket_key: "dm-peer-person.neoragex2002".into(),
            }
        );
    }

    #[test]
    fn parse_accepts_system_cron_session_key() {
        assert_eq!(
            SessionKey::parse("system:cron:heartbeat").unwrap(),
            ParsedSessionKey::System {
                service_id: "cron".into(),
                bucket_key: "heartbeat".into(),
            }
        );
    }

    #[test]
    fn parse_rejects_non_canonical_shapes() {
        assert!(SessionKey::parse("main").is_err());
        assert!(SessionKey::parse("cron:heartbeat").is_err());
        assert!(SessionKey::parse("dm:main").is_err());
    }
}
