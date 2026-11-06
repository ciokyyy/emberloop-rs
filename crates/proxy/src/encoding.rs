//! Bounded request decoding and re-encoding for model wires that send compressed JSON.
use axum::body::Bytes;
use std::io::Read;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("decoded request body too large")]
    TooLarge,
    #[error("unsupported request content encoding: {0}")]
    Unsupported(String),
    #[error("invalid compressed request body: {0}")]
    Invalid(#[from] std::io::Error),
}

pub fn decode(encoding: Option<&str>, body: Bytes, limit: usize) -> Result<Bytes, Error> {
    match encoding.unwrap_or("identity").trim() {
        "identity" | "" => Ok(body),
        "zstd" => {
            let decoder = zstd::stream::read::Decoder::new(body.as_ref())?;
            let mut decoded = Vec::new();
            decoder
                .take(limit.saturating_add(1) as u64)
                .read_to_end(&mut decoded)?;
            if decoded.len() > limit {
                return Err(Error::TooLarge);
            }
            Ok(decoded.into())
        }
        other => Err(Error::Unsupported(other.to_string())),
    }
}

pub fn encode(encoding: Option<&str>, body: Vec<u8>) -> Result<Vec<u8>, Error> {
    match encoding.unwrap_or("identity").trim() {
        "identity" | "" => Ok(body),
        "zstd" => Ok(zstd::stream::encode_all(body.as_slice(), 1)?),
        other => Err(Error::Unsupported(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_requests_are_decoded_and_reencoded_after_mutation() {
        let json = br#"{"input":[]}"#.to_vec();
        let encoded = encode(Some("zstd"), json.clone()).unwrap();
        assert_ne!(encoded, json);
        assert_eq!(
            decode(Some("zstd"), encoded.into(), 1024).unwrap().as_ref(),
            json
        );
    }

    #[test]
    fn decoded_size_is_bounded_independently_of_compressed_size() {
        let encoded = zstd::stream::encode_all(&b"x".repeat(4096)[..], 1).unwrap();
        assert!(matches!(
            decode(Some("zstd"), encoded.into(), 32),
            Err(Error::TooLarge)
        ));
        assert!(decode(Some("zstd"), b"broken".as_slice().into(), 32).is_err());
        assert!(decode(Some("gzip"), b"broken".as_slice().into(), 32).is_err());
    }
}
