use super::OpenAiResponsesBackendError as Error;

// Includes ignored fields and comments within a frame, not just JSON data.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct Decoder {
    line: Vec<u8>,
    data: String,
    has_data: bool,
    skip_lf: bool,
    frame_bytes: usize,
}

impl Decoder {
    // Consume only through the next event. The caller retains the transport
    // chunk and applies observer backpressure before decoding further events.
    pub(super) fn next(&mut self, input: &mut &[u8]) -> Result<Option<String>, Error> {
        while let Some((&byte, rest)) = input.split_first() {
            *input = rest;
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.frame_bytes += 1;
            if self.frame_bytes > MAX_FRAME_BYTES {
                return Err(Error::MalformedStream("SSE frame exceeded the size limit"));
            }
            if byte != b'\r' && byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            self.skip_lf = byte == b'\r';
            let line = std::str::from_utf8(&self.line)
                .map_err(|_| Error::MalformedStream("stream was not UTF-8"))?;
            if line.is_empty() {
                self.frame_bytes = 0;
                if self.has_data {
                    self.has_data = false;
                    return Ok(Some(std::mem::take(&mut self.data)));
                }
            } else {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                if field == "data" {
                    if self.has_data {
                        self.data.push('\n');
                    }
                    self.has_data = true;
                    self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
                }
            }
            self.line.clear();
        }
        Ok(None)
    }

    pub(super) fn finish(self) -> Result<(), Error> {
        if !self.line.is_empty() || self.has_data {
            return Err(Error::MalformedStream(
                "stream ended before an SSE event boundary",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&[u8]]) -> Result<Vec<String>, Error> {
        let mut decoder = Decoder::default();
        let mut result = Vec::new();
        for mut chunk in chunks.iter().copied() {
            while let Some(event) = decoder.next(&mut chunk)? {
                result.push(event);
            }
        }
        decoder.finish()?;
        Ok(result)
    }

    #[test]
    fn arbitrary_transport_splits_preserve_utf8_crlf_and_multiline_data() {
        let bytes =
            ": comment\r\nevent: test\r\ndata: 你好\r\ndata: world\r\n\r\ndata: end\r\r".as_bytes();
        for split in 0..=bytes.len() {
            assert_eq!(
                decode(&[&bytes[..split], &bytes[split..]]).unwrap(),
                ["你好\nworld", "end"]
            );
        }
        let chunks = bytes.chunks(1).collect::<Vec<_>>();
        assert_eq!(decode(&chunks).unwrap(), ["你好\nworld", "end"]);
    }

    #[test]
    fn comments_fields_and_empty_data_follow_sse_framing() {
        assert_eq!(
            decode(&[b": hi\n\ndata\n\ndata: x\n\n: trailing\n"]).unwrap(),
            ["", "x"]
        );
        assert!(decode(&[b"data: x\n"]).is_err());
        assert!(decode(&[b"data: x"]).is_err());
        assert!(decode(&[b"data: \xff\n\n"]).is_err());
    }

    #[test]
    fn bounds_each_frame_including_ignored_lines_before_copying_large_chunks() {
        let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut decoder = Decoder::default();
        assert!(decoder.next(&mut oversized.as_slice()).is_err());
        assert_eq!(decoder.line.len(), MAX_FRAME_BYTES);
        let ignored = b": a\n".repeat(MAX_FRAME_BYTES / 4 + 1);
        assert!(decode(&[&ignored]).is_err());
        let data = b"data: x\n".repeat(MAX_FRAME_BYTES / 8 + 1);
        assert!(decode(&[&data]).is_err());
        let many = b"data: x\n\n".repeat(MAX_FRAME_BYTES / 9 + 1);
        assert_eq!(decode(&[&many]).unwrap().len(), MAX_FRAME_BYTES / 9 + 1);
    }
}
