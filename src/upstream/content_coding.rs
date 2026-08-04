//! Streaming upstream HTTP content-coding negotiation and decoding.

use std::{error::Error, io, pin::Pin};

use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::{CONTENT_ENCODING, HeaderMap};
use thiserror::Error;
use tokio::io::{AsyncRead, BufReader};
use tokio_util::io::{ReaderStream, StreamReader};

pub(crate) const UPSTREAM_ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";
const DECODED_CHUNK_BYTES: usize = 64 * 1024;
const MAX_CONTENT_CODINGS: usize = 4;

pub(crate) type DecodedBodyError = Box<dyn Error + Send + Sync>;
pub(crate) type DecodedBodyStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, DecodedBodyError>> + Send>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentCoding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// Ordered content codings applied by the upstream response.
///
/// HTTP applies codings in header order, so decoding wraps readers in reverse
/// order. An empty list is the identity representation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResponseContentCodings {
    codings: Box<[ContentCoding]>,
}

impl ResponseContentCodings {
    pub(crate) fn parse(headers: &HeaderMap) -> Result<Self, UnsupportedContentEncoding> {
        let mut codings = Vec::new();
        for value in headers.get_all(CONTENT_ENCODING) {
            let value = value.to_str().map_err(|_| UnsupportedContentEncoding)?;
            for token in value.split(',') {
                let token = token.trim();
                if token.is_empty() {
                    return Err(UnsupportedContentEncoding);
                }
                let coding = match token.to_ascii_lowercase().as_str() {
                    "identity" => continue,
                    "gzip" | "x-gzip" => ContentCoding::Gzip,
                    "deflate" => ContentCoding::Deflate,
                    "br" => ContentCoding::Brotli,
                    "zstd" => ContentCoding::Zstd,
                    _ => return Err(UnsupportedContentEncoding),
                };
                codings.push(coding);
                if codings.len() > MAX_CONTENT_CODINGS {
                    return Err(UnsupportedContentEncoding);
                }
            }
        }
        Ok(Self {
            codings: codings.into_boxed_slice(),
        })
    }

    pub(crate) fn is_encoded(&self) -> bool {
        !self.codings.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("unsupported upstream response content encoding")]
pub(crate) struct UnsupportedContentEncoding;

pub(crate) fn decode_response_body(
    response: reqwest::Response,
    codings: &ResponseContentCodings,
) -> DecodedBodyStream {
    if !codings.is_encoded() {
        return Box::pin(
            response
                .bytes_stream()
                .map(|result| result.map_err(|error| Box::new(error) as DecodedBodyError)),
        );
    }

    let raw = response
        .bytes_stream()
        .map(|result| result.map_err(io::Error::other));
    let mut reader: Box<dyn AsyncRead + Send + Unpin> = Box::new(StreamReader::new(raw));
    for coding in codings.codings.iter().rev() {
        let buffered = BufReader::new(reader);
        reader = match coding {
            ContentCoding::Gzip => Box::new(GzipDecoder::new(buffered)),
            ContentCoding::Deflate => Box::new(ZlibDecoder::new(buffered)),
            ContentCoding::Brotli => Box::new(BrotliDecoder::new(buffered)),
            ContentCoding::Zstd => {
                let mut decoder = ZstdDecoder::new(buffered);
                decoder.multiple_members(true);
                Box::new(decoder)
            }
        };
    }
    Box::pin(
        ReaderStream::with_capacity(reader, DECODED_CHUNK_BYTES)
            .map(|result| result.map_err(|error| Box::new(error) as DecodedBodyError)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn parses_identity_single_and_stacked_content_codings() {
        assert_eq!(
            ResponseContentCodings::parse(&HeaderMap::new()).unwrap(),
            ResponseContentCodings::default()
        );

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("br"));
        assert_eq!(
            ResponseContentCodings::parse(&headers).unwrap(),
            ResponseContentCodings {
                codings: vec![ContentCoding::Brotli].into_boxed_slice(),
            }
        );

        headers.insert(
            CONTENT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );
        assert_eq!(
            ResponseContentCodings::parse(&headers).unwrap(),
            ResponseContentCodings {
                codings: vec![
                    ContentCoding::Gzip,
                    ContentCoding::Deflate,
                    ContentCoding::Brotli,
                    ContentCoding::Zstd,
                ]
                .into_boxed_slice(),
            }
        );
    }

    #[test]
    fn rejects_unknown_invalid_or_excessive_content_codings() {
        for value in ["compress", "gzip,", "gzip, deflate, br, zstd, gzip"] {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static(value));
            assert_eq!(
                ResponseContentCodings::parse(&headers),
                Err(UnsupportedContentEncoding)
            );
        }
    }
}
