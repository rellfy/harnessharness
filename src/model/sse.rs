const EVENT_SEPARATOR: &[u8] = b"\n\n";
const DATA_FIELD: &str = "data:";

/// Splits a server-sent event byte stream into the data payloads of complete events.
#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer
            .extend(bytes.iter().copied().filter(|byte| *byte != b'\r'));
        let mut payloads = Vec::new();
        while let Some(end) = find_separator(&self.buffer) {
            let event: Vec<u8> = self.buffer.drain(..end + EVENT_SEPARATOR.len()).collect();
            if let Some(payload) = read_data(&event[..end]) {
                payloads.push(payload);
            }
        }
        payloads
    }
}

fn find_separator(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(EVENT_SEPARATOR.len())
        .position(|window| window == EVENT_SEPARATOR)
}

fn read_data(event: &[u8]) -> Option<String> {
    let event = String::from_utf8_lossy(event);
    let data_lines: Vec<&str> = event
        .lines()
        .filter_map(|line| line.strip_prefix(DATA_FIELD))
        .map(|data| data.strip_prefix(' ').unwrap_or(data))
        .collect();
    match data_lines.is_empty() {
        true => None,
        false => Some(data_lines.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_events_split_across_chunks() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"event: ping\ndata: {\"a\"").is_empty());
        assert_eq!(decoder.push(b":1}\n\nevent: x\n"), ["{\"a\":1}"]);
        assert_eq!(decoder.push(b"data: 2\n\n"), ["2"]);
    }

    #[test]
    fn handles_crlf_comments_and_multiline_data() {
        let mut decoder = SseDecoder::default();
        let payloads = decoder.push(b": keepalive\r\n\r\ndata: a\r\ndata:b\r\n\r\n");
        assert_eq!(payloads, ["a\nb"]);
    }

    #[test]
    fn keeps_multibyte_characters_split_across_chunks() {
        let mut decoder = SseDecoder::default();
        let bytes = "data: é\n\n".as_bytes();
        assert!(decoder.push(&bytes[..7]).is_empty());
        assert_eq!(decoder.push(&bytes[7..]), ["é"]);
    }
}
