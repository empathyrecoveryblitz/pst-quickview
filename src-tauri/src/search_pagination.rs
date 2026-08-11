use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::search::MessageSort;

pub(crate) const INVALID_SEARCH_CURSOR_CODE: &str = "INVALID_SEARCH_CURSOR";
pub(crate) const STALE_SEARCH_CURSOR_CODE: &str = "STALE_SEARCH_CURSOR";
pub(crate) const UNSUPPORTED_SEARCH_CURSOR_CODE: &str = "UNSUPPORTED_SEARCH_CURSOR";

const CURSOR_PREFIX: &str = "pqv-msg-v1.";
const CURSOR_FAMILY_PREFIX: &str = "pqv-msg-v";
const CURSOR_VERSION: u8 = 1;
const CURSOR_TAG_BYTES: usize = 16;
const CURSOR_PAYLOAD_BYTES: usize = 66;
const CURSOR_BYTES: usize = CURSOR_PAYLOAD_BYTES + CURSOR_TAG_BYTES;
const MAX_CURSOR_SCALARS: usize = 192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MessageCursorContext {
    pub(crate) workspace_hash: [u8; 16],
    pub(crate) criteria_hash: [u8; 16],
    pub(crate) index_generation: [u8; 16],
    pub(crate) sort: MessageSort,
    pub(crate) search_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MessageCursorPosition {
    pub(crate) message_id: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchCursorError {
    Invalid,
    Stale,
    Unsupported,
}

impl SearchCursorError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Invalid => INVALID_SEARCH_CURSOR_CODE,
            Self::Stale => STALE_SEARCH_CURSOR_CODE,
            Self::Unsupported => UNSUPPORTED_SEARCH_CURSOR_CODE,
        }
    }
}

impl std::fmt::Display for SearchCursorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "The search cursor is invalid for this request.",
            Self::Stale => "The search cursor is stale because the workspace index changed.",
            Self::Unsupported => "This search path does not support the supplied cursor.",
        })
    }
}

impl std::error::Error for SearchCursorError {}

#[derive(Clone)]
pub(crate) struct SearchCursorCodec {
    secret: [u8; 32],
}

impl Default for SearchCursorCodec {
    fn default() -> Self {
        Self {
            secret: process_cursor_secret(),
        }
    }
}

impl SearchCursorCodec {
    pub(crate) fn encode(
        &self,
        context: &MessageCursorContext,
        position: MessageCursorPosition,
    ) -> String {
        let mut payload = Vec::with_capacity(CURSOR_PAYLOAD_BYTES);
        payload.push(CURSOR_VERSION);
        payload.push(context.sort.cursor_tag());
        payload.extend_from_slice(&context.search_generation.to_be_bytes());
        payload.extend_from_slice(&position.message_id.to_be_bytes());
        payload.extend_from_slice(&context.workspace_hash);
        payload.extend_from_slice(&context.criteria_hash);
        payload.extend_from_slice(&context.index_generation);
        debug_assert_eq!(payload.len(), CURSOR_PAYLOAD_BYTES);

        let tag = self.authentication_tag(&payload);
        payload.extend_from_slice(&tag);
        format!("{CURSOR_PREFIX}{}", hex::encode(payload))
    }

    pub(crate) fn decode(
        &self,
        encoded: &str,
        expected: &MessageCursorContext,
    ) -> Result<MessageCursorPosition, SearchCursorError> {
        if encoded.chars().count() > MAX_CURSOR_SCALARS {
            return Err(SearchCursorError::Invalid);
        }
        let Some(body) = encoded.strip_prefix(CURSOR_PREFIX) else {
            return Err(if has_unsupported_cursor_version(encoded) {
                SearchCursorError::Unsupported
            } else {
                SearchCursorError::Invalid
            });
        };
        if body.len() != CURSOR_BYTES * 2 || !body.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SearchCursorError::Invalid);
        }

        let decoded = hex::decode(body).map_err(|_| SearchCursorError::Invalid)?;
        if decoded.len() != CURSOR_BYTES {
            return Err(SearchCursorError::Invalid);
        }
        let (payload, supplied_tag) = decoded.split_at(CURSOR_PAYLOAD_BYTES);
        if !tags_equal(&self.authentication_tag(payload), supplied_tag) {
            return Err(SearchCursorError::Invalid);
        }
        if payload[0] != CURSOR_VERSION {
            return Err(SearchCursorError::Unsupported);
        }

        let sort =
            MessageSort::from_cursor_tag(payload[1]).ok_or(SearchCursorError::Unsupported)?;
        let search_generation = u64::from_be_bytes(
            payload[2..10]
                .try_into()
                .map_err(|_| SearchCursorError::Invalid)?,
        );
        let message_id = i64::from_be_bytes(
            payload[10..18]
                .try_into()
                .map_err(|_| SearchCursorError::Invalid)?,
        );
        let workspace_hash = fixed_16(&payload[18..34])?;
        let criteria_hash = fixed_16(&payload[34..50])?;
        let index_generation = fixed_16(&payload[50..66])?;

        if message_id <= 0
            || sort != expected.sort
            || search_generation != expected.search_generation
            || workspace_hash != expected.workspace_hash
            || criteria_hash != expected.criteria_hash
        {
            return Err(SearchCursorError::Invalid);
        }
        if index_generation != expected.index_generation {
            return Err(SearchCursorError::Stale);
        }

        Ok(MessageCursorPosition { message_id })
    }

    fn authentication_tag(&self, payload: &[u8]) -> [u8; CURSOR_TAG_BYTES] {
        let mut hasher = Sha256::new();
        hasher.update(b"pst-quickview-message-cursor-v1");
        hasher.update(self.secret);
        hasher.update(payload);
        let digest = hasher.finalize();
        let mut tag = [0u8; CURSOR_TAG_BYTES];
        tag.copy_from_slice(&digest[..CURSOR_TAG_BYTES]);
        tag
    }

    #[cfg(test)]
    fn with_secret(secret: [u8; 32]) -> Self {
        Self { secret }
    }
}

pub(crate) fn opaque_hash(value: &[u8]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"pst-quickview-cursor-component-v1");
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
    let digest = hasher.finalize();
    let mut result = [0u8; 16];
    result.copy_from_slice(&digest[..16]);
    result
}

fn has_unsupported_cursor_version(value: &str) -> bool {
    value
        .strip_prefix(CURSOR_FAMILY_PREFIX)
        .and_then(|suffix| suffix.split_once('.'))
        .is_some_and(|(version, _)| {
            !version.is_empty()
                && version.bytes().all(|byte| byte.is_ascii_digit())
                && version != "1"
        })
}

fn fixed_16(value: &[u8]) -> Result<[u8; 16], SearchCursorError> {
    value.try_into().map_err(|_| SearchCursorError::Invalid)
}

fn tags_equal(expected: &[u8; CURSOR_TAG_BYTES], supplied: &[u8]) -> bool {
    if supplied.len() != expected.len() {
        return false;
    }
    expected
        .iter()
        .zip(supplied)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn process_cursor_secret() -> [u8; 32] {
    let mut secret = [0u8; 32];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut secret))
        .is_ok()
    {
        return secret;
    }

    let mut hasher = Sha256::new();
    hasher.update(b"pst-quickview-cursor-fallback-secret");
    hasher.update(process::id().to_be_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_be_bytes(),
    );
    secret.copy_from_slice(&hasher.finalize());
    secret
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> MessageCursorContext {
        MessageCursorContext {
            workspace_hash: [1; 16],
            criteria_hash: [2; 16],
            index_generation: [3; 16],
            sort: MessageSort::Newest,
            search_generation: 7,
        }
    }

    #[test]
    fn cursor_round_trip_is_bounded_and_contains_no_cleartext() {
        let codec = SearchCursorCodec::with_secret([9; 32]);
        let encoded = codec.encode(&context(), MessageCursorPosition { message_id: 42 });
        assert!(encoded.len() <= MAX_CURSOR_SCALARS);
        assert!(encoded.starts_with(CURSOR_PREFIX));
        assert_eq!(
            codec.decode(&encoded, &context()).unwrap(),
            MessageCursorPosition { message_id: 42 }
        );
        for private_value in ["query phrase", "/private/workspace", "message subject"] {
            assert!(!encoded.contains(private_value));
        }
    }

    #[test]
    fn malformed_truncated_oversized_and_hostile_values_never_panic() {
        let codec = SearchCursorCodec::with_secret([9; 32]);
        let values = [
            "",
            "not-a-cursor",
            "pqv-msg-v1",
            CURSOR_PREFIX,
            "pqv-msg-v1.zz",
            &"x".repeat(MAX_CURSOR_SCALARS + 1),
        ];
        for value in values {
            let result = std::panic::catch_unwind(|| codec.decode(value, &context()));
            assert!(result.is_ok(), "cursor parsing must not panic");
            assert_eq!(result.unwrap(), Err(SearchCursorError::Invalid));
        }
    }

    #[test]
    fn unsupported_version_is_distinct() {
        let codec = SearchCursorCodec::with_secret([9; 32]);
        assert_eq!(
            codec.decode("pqv-msg-v2.deadbeef", &context()),
            Err(SearchCursorError::Unsupported)
        );
    }

    #[test]
    fn tampering_wrong_context_and_stale_index_are_rejected() {
        let codec = SearchCursorCodec::with_secret([9; 32]);
        let encoded = codec.encode(&context(), MessageCursorPosition { message_id: 42 });

        let mut tampered = encoded.clone().into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'0' { b'1' } else { b'0' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert_eq!(
            codec.decode(&tampered, &context()),
            Err(SearchCursorError::Invalid)
        );

        let mut wrong_workspace = context();
        wrong_workspace.workspace_hash = [4; 16];
        assert_eq!(
            codec.decode(&encoded, &wrong_workspace),
            Err(SearchCursorError::Invalid)
        );

        let mut wrong_sort = context();
        wrong_sort.sort = MessageSort::Oldest;
        assert_eq!(
            codec.decode(&encoded, &wrong_sort),
            Err(SearchCursorError::Invalid)
        );

        let mut stale = context();
        stale.index_generation = [5; 16];
        assert_eq!(
            codec.decode(&encoded, &stale),
            Err(SearchCursorError::Stale)
        );
    }
}
