// glTF buffers and images may be `data:` URIs; base64 is the only encoding the format allows there.

pub(super) fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let payload = uri.strip_prefix("data:")?;
    let (header, encoded) = payload.split_once(',')?;
    header.ends_with(";base64").then(|| decode_base64(encoded))?
}

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let (mut accumulator, mut bits) = (0_u32, 0_u32);
    for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        accumulator = (accumulator << 6) | u32::from(sextet(byte)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((accumulator >> bits) & 0xff).ok()?);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::decode_data_uri;

    #[test]
    fn decodes_padded_and_unpadded_base64() {
        assert_eq!(
            decode_data_uri("data:text/plain;base64,aGVsbG8=").as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(decode_data_uri("data:;base64,aGVsbG8").as_deref(), Some(&b"hello"[..]));
    }

    #[test]
    fn rejects_other_encodings_and_garbage() {
        assert_eq!(decode_data_uri("data:text/plain,hello"), None);
        assert_eq!(decode_data_uri("data:;base64,a$b"), None);
        assert_eq!(decode_data_uri("file.bin"), None);
    }
}
