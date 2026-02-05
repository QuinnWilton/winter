//! AT URI parsing utilities.

use std::fmt;

use thiserror::Error;

/// Error when parsing an AT URI.
#[derive(Debug, Error)]
#[error("invalid AT URI: {0}")]
pub struct AtUriError(String);

/// A parsed AT Protocol URI.
///
/// AT URIs have the format: `at://{did}/{collection}/{rkey}`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AtUri {
    /// The DID of the repository owner.
    pub did: String,
    /// The collection (e.g., "app.bsky.feed.post").
    pub collection: String,
    /// The record key.
    pub rkey: String,
}

impl AtUri {
    /// Parse an AT URI string.
    ///
    /// # Example
    ///
    /// ```
    /// use winter_atproto::AtUri;
    ///
    /// let uri = AtUri::parse("at://did:plc:abc123/app.bsky.feed.post/3abc").unwrap();
    /// assert_eq!(uri.did, "did:plc:abc123");
    /// assert_eq!(uri.collection, "app.bsky.feed.post");
    /// assert_eq!(uri.rkey, "3abc");
    /// ```
    pub fn parse(uri: &str) -> Result<Self, AtUriError> {
        let rest = uri
            .strip_prefix("at://")
            .ok_or_else(|| AtUriError(format!("missing at:// prefix: {uri}")))?;

        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(AtUriError(format!("expected did/collection/rkey: {uri}")));
        }

        if parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
            return Err(AtUriError(format!("empty component in URI: {uri}")));
        }

        Ok(Self {
            did: parts[0].to_string(),
            collection: parts[1].to_string(),
            rkey: parts[2].to_string(),
        })
    }

    /// Quick rkey extraction without full parsing.
    ///
    /// This is a backwards-compatible fallback that extracts the last path
    /// component from any URI-like string. Returns an empty string if no
    /// slash is found.
    ///
    /// # Example
    ///
    /// ```
    /// use winter_atproto::AtUri;
    ///
    /// assert_eq!(AtUri::extract_rkey("at://did/col/rkey123"), "rkey123");
    /// assert_eq!(AtUri::extract_rkey("some/path/to/key"), "key");
    /// assert_eq!(AtUri::extract_rkey("no-slash"), "no-slash");
    /// ```
    pub fn extract_rkey(uri: &str) -> &str {
        uri.rsplit('/').next().unwrap_or("")
    }
}

impl fmt::Display for AtUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at://{}/{}/{}", self.did, self.collection, self.rkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_uri() {
        let uri = AtUri::parse("at://did:plc:abc123/app.bsky.feed.post/3abc").unwrap();
        assert_eq!(uri.did, "did:plc:abc123");
        assert_eq!(uri.collection, "app.bsky.feed.post");
        assert_eq!(uri.rkey, "3abc");
    }

    #[test]
    fn test_parse_winter_uri() {
        let uri = AtUri::parse("at://did:plc:xyz/diy.razorgirl.winter.fact/3jqfcqzhs3u2v").unwrap();
        assert_eq!(uri.did, "did:plc:xyz");
        assert_eq!(uri.collection, "diy.razorgirl.winter.fact");
        assert_eq!(uri.rkey, "3jqfcqzhs3u2v");
    }

    #[test]
    fn test_parse_missing_prefix() {
        let result = AtUri::parse("did:plc:abc/collection/rkey");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing at:// prefix")
        );
    }

    #[test]
    fn test_parse_missing_rkey() {
        let result = AtUri::parse("at://did:plc:abc/collection");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_component() {
        let result = AtUri::parse("at://did:plc:abc//rkey");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty component"));
    }

    #[test]
    fn test_display() {
        let uri = AtUri {
            did: "did:plc:test".to_string(),
            collection: "app.bsky.feed.post".to_string(),
            rkey: "abc123".to_string(),
        };
        assert_eq!(
            uri.to_string(),
            "at://did:plc:test/app.bsky.feed.post/abc123"
        );
    }

    #[test]
    fn test_extract_rkey_at_uri() {
        assert_eq!(
            AtUri::extract_rkey("at://did:plc:abc/diy.razorgirl.winter.fact/3abc123"),
            "3abc123"
        );
    }

    #[test]
    fn test_extract_rkey_simple_path() {
        assert_eq!(AtUri::extract_rkey("some/path/to/key"), "key");
    }

    #[test]
    fn test_extract_rkey_no_slash() {
        assert_eq!(AtUri::extract_rkey("no-slash"), "no-slash");
    }

    #[test]
    fn test_extract_rkey_empty() {
        assert_eq!(AtUri::extract_rkey(""), "");
    }

    #[test]
    fn test_extract_rkey_trailing_slash() {
        assert_eq!(AtUri::extract_rkey("path/to/"), "");
    }

    #[test]
    fn test_roundtrip() {
        let original = "at://did:plc:abc123/app.bsky.feed.post/xyz789";
        let parsed = AtUri::parse(original).unwrap();
        assert_eq!(parsed.to_string(), original);
    }
}
