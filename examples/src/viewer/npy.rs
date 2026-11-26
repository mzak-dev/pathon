// NumPy `.npy` version 1.0, little-endian float32, C order: the format `tools/scenes/compare.py` reads with `np.load`.

const MAGIC: &[u8] = b"\x93NUMPY\x01\x00";
const HEADER_ALIGNMENT: usize = 64;

/// The file for a float32 array of `shape`; the header is padded with spaces to a multiple of 64 bytes and ends in a
/// newline.
pub fn encode_f32(shape: &[usize], values: &[f32]) -> Vec<u8> {
    let dims: Vec<String> = shape.iter().map(usize::to_string).collect();
    let tuple = if dims.len() == 1 {
        format!("({},)", dims[0])
    } else {
        format!("({})", dims.join(", "))
    };
    let mut header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': {tuple}, }}");
    let unpadded = MAGIC.len() + 2 + header.len() + 1;
    header.push_str(&" ".repeat(unpadded.next_multiple_of(HEADER_ALIGNMENT) - unpadded));
    header.push('\n');
    let mut bytes = Vec::with_capacity(MAGIC.len() + 2 + header.len() + values.len() * 4);
    bytes.extend_from_slice(MAGIC);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a version 1.0 header is at most 65535 bytes, ours is a few dozen"
    )]
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Reads what [`encode_f32`] writes (and any version 1.0 little-endian float32 C-order file): the shape and the values.
pub fn decode_f32(bytes: &[u8]) -> Result<(Vec<usize>, Vec<f32>), String> {
    if bytes.len() < 10 || &bytes[..8] != MAGIC {
        return Err("not a NumPy version 1.0 file".into());
    }
    let header_length = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
    let data_start = 10 + header_length;
    let header = bytes
        .get(10..data_start)
        .and_then(|header| std::str::from_utf8(header).ok())
        .ok_or("the header is cut off")?;
    if !header.contains("'<f4'") || !header.contains("'fortran_order': False") {
        return Err(format!(
            "only little-endian float32 C-order arrays are read, got {header}"
        ));
    }
    let shape_text = header
        .split("'shape': (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .ok_or("the header has no shape")?;
    let shape = shape_text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<usize>().map_err(|_| format!("bad dimension '{part}'")))
        .collect::<Result<Vec<_>, _>>()?;
    let data = &bytes[data_start..];
    if data.len() != shape.iter().product::<usize>() * 4 {
        return Err("the data does not match the shape".into());
    }
    let values = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    Ok((shape, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_aligned_and_spells_the_shape_like_numpy_does() {
        let bytes = encode_f32(&[2, 3, 3], &[0.0; 18]);
        assert_eq!(&bytes[..8], b"\x93NUMPY\x01\x00");
        let data_start = 10 + usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        assert_eq!(data_start % 64, 0);
        assert_eq!(bytes.len(), data_start + 18 * 4);
        let header = std::str::from_utf8(&bytes[10..data_start]).expect("ascii header");
        assert!(header.starts_with("{'descr': '<f4', 'fortran_order': False, 'shape': (2, 3, 3), }"));
        assert!(header.ends_with(" \n") || header.ends_with("}\n"));
    }

    #[test]
    fn an_array_survives_a_round_trip() {
        let values = [0.0, 1.5, -2.25, 683.0, f32::MIN_POSITIVE, 65504.0];
        let (shape, back) = decode_f32(&encode_f32(&[1, 2, 3], &values)).expect("decodes");
        assert_eq!(shape, vec![1, 2, 3]);
        assert_eq!(back, values);
    }

    #[test]
    fn a_one_dimensional_shape_has_the_trailing_comma_of_a_python_tuple() {
        let bytes = encode_f32(&[4], &[0.0; 4]);
        assert!(
            std::str::from_utf8(&bytes[10..])
                .expect("ascii header")
                .contains("'shape': (4,), }")
        );
    }

    #[test]
    fn other_files_are_refused() {
        assert!(decode_f32(b"not numpy at all").is_err());
        let mut truncated = encode_f32(&[1, 1, 3], &[1.0, 2.0, 3.0]);
        truncated.pop();
        assert!(decode_f32(&truncated).is_err());
    }
}
