use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-signed Session token: `hex(session_id).hex(signature)`.
///
/// The token carries the Session id itself, so the proxy needs no header to
/// know which Session a model request belongs to. The path keeps the signature
/// scoped to the Session id and secret only.
#[derive(Clone)]
pub struct TokenSigner {
    secret: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("malformed session token")]
    Malformed,
    #[error("invalid session token")]
    Invalid,
}

impl TokenSigner {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    pub fn sign(&self, session_id: &str) -> String {
        let Some(mut mac) = self.mac() else {
            return String::new();
        };
        mac.update(session_id.as_bytes());
        format!(
            "{}.{}",
            hex::encode(session_id.as_bytes()),
            hex::encode(mac.finalize().into_bytes())
        )
    }

    pub fn verify(&self, token: &str) -> Result<String, TokenError> {
        let (payload, signature) = token.split_once('.').ok_or(TokenError::Malformed)?;
        let payload = hex::decode(payload).map_err(|_| TokenError::Malformed)?;
        let signature = hex::decode(signature).map_err(|_| TokenError::Malformed)?;
        let session_id = String::from_utf8(payload).map_err(|_| TokenError::Malformed)?;

        let Some(mut mac) = self.mac() else {
            return Err(TokenError::Invalid);
        };
        mac.update(session_id.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| TokenError::Invalid)?;
        Ok(session_id)
    }

    fn mac(&self) -> Option<HmacSha256> {
        HmacSha256::new_from_slice(&self.secret).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trips_the_session_id() {
        let signer = TokenSigner::new(b"secret".to_vec());
        let token = signer.sign("ses_abc");
        assert_eq!(signer.verify(&token).unwrap(), "ses_abc");
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let signer = TokenSigner::new(b"secret".to_vec());
        let token = signer.sign("ses_abc");
        let (_, signature) = token.split_once('.').unwrap();
        let forged = format!("{}.{}", hex::encode("ses_other"), signature);
        assert_eq!(signer.verify(&forged), Err(TokenError::Invalid));
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let token = TokenSigner::new(b"secret".to_vec()).sign("ses_abc");
        let other = TokenSigner::new(b"another".to_vec());
        assert_eq!(other.verify(&token), Err(TokenError::Invalid));
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let signer = TokenSigner::new(b"secret".to_vec());
        assert_eq!(signer.verify("nope"), Err(TokenError::Malformed));
        assert_eq!(signer.verify("zz.zz"), Err(TokenError::Malformed));
        assert_eq!(signer.verify("abc"), Err(TokenError::Malformed));
    }
}
