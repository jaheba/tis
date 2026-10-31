//! Binary-to-text codecs independent of VM frames and object ownership.
use crate::{Error, Result};
use base64::Engine;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const B85: &[u8; 85] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
fn invalid() -> Error {
    Error::Runtime("ValueError: invalid encoded data".into())
}
pub(crate) fn encode(kind: &str, data: &[u8], pad: bool) -> Result<Vec<u8>> {
    Ok(match kind {
        "b64encode" => base64::engine::general_purpose::STANDARD
            .encode(data)
            .into_bytes(),
        "urlsafe_b64encode" => base64::engine::general_purpose::URL_SAFE
            .encode(data)
            .into_bytes(),
        "b16encode" => data
            .iter()
            .flat_map(|byte| {
                [
                    b"0123456789ABCDEF"[(byte >> 4) as usize],
                    b"0123456789ABCDEF"[(byte & 15) as usize],
                ]
            })
            .collect(),
        "b32encode" => {
            let mut result = Vec::new();
            let (mut acc, mut bits) = (0u32, 0);
            for &byte in data {
                acc = (acc << 8) | u32::from(byte);
                bits += 8;
                while bits >= 5 {
                    bits -= 5;
                    result.push(B32[((acc >> bits) & 31) as usize]);
                }
                acc &= (1 << bits) - 1;
            }
            if bits != 0 {
                result.push(B32[((acc << (5 - bits)) & 31) as usize]);
            }
            while result.len() % 8 != 0 {
                result.push(b'=');
            }
            result
        }
        "b85encode" | "a85encode" => {
            let ascii = kind == "a85encode";
            let mut result = Vec::new();
            for chunk in data.chunks(4) {
                let mut bytes = [0u8; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                let mut value = u32::from_be_bytes(bytes);
                if ascii && value == 0 && chunk.len() == 4 {
                    result.push(b'z');
                    continue;
                }
                let mut group = [0u8; 5];
                for i in (0..5).rev() {
                    let digit = (value % 85) as usize;
                    value /= 85;
                    group[i] = if ascii {
                        digit as u8 + b'!'
                    } else {
                        B85[digit]
                    };
                }
                result.extend_from_slice(&group[..if pad { 5 } else { chunk.len() + 1 }]);
            }
            result
        }
        _ => return Err(Error::Unsupported("base encoding")),
    })
}
pub(crate) fn decode(kind: &str, data: &[u8], casefold: bool, validate: bool) -> Result<Vec<u8>> {
    match kind {
        "b64decode" | "urlsafe_b64decode" => {
            let mut data = data.to_vec();
            if kind == "urlsafe_b64decode" {
                for b in &mut data {
                    if *b == b'-' {
                        *b = b'+';
                    } else if *b == b'_' {
                        *b = b'/';
                    }
                }
            }
            if !validate {
                data.retain(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
            }
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| invalid())
        }
        "b16decode" => {
            if !data.len().is_multiple_of(2) {
                return Err(invalid());
            }
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let digit = |mut b: u8| {
                        if casefold {
                            b = b.to_ascii_uppercase();
                        }
                        match b {
                            b'0'..=b'9' => Ok(b - b'0'),
                            b'A'..=b'F' => Ok(b - b'A' + 10),
                            _ => Err(invalid()),
                        }
                    };
                    Ok(digit(pair[0])? * 16 + digit(pair[1])?)
                })
                .collect()
        }
        "b32decode" => {
            if !data.len().is_multiple_of(8) {
                return Err(invalid());
            }
            let count = data.iter().position(|&b| b == b'=').unwrap_or(data.len());
            if !matches!(data.len() - count, 0 | 1 | 3 | 4 | 6)
                || data[count..].iter().any(|&b| b != b'=')
            {
                return Err(invalid());
            }
            let (mut acc, mut bits) = (0u32, 0);
            let mut result = Vec::new();
            for &byte in &data[..count] {
                let byte = if casefold {
                    byte.to_ascii_uppercase()
                } else {
                    byte
                };
                let digit = B32.iter().position(|&b| b == byte).ok_or_else(invalid)? as u32;
                acc = (acc << 5) | digit;
                bits += 5;
                if bits >= 8 {
                    bits -= 8;
                    result.push((acc >> bits) as u8);
                }
                acc &= (1 << bits) - 1;
            }
            Ok(result)
        }
        "a85decode" | "b85decode" => {
            let ascii = kind == "a85decode";
            let mut result = Vec::new();
            let mut group = Vec::with_capacity(5);
            for &byte in data {
                if ascii && byte.is_ascii_whitespace() {
                    continue;
                }
                if ascii && byte == b'z' {
                    if !group.is_empty() {
                        return Err(invalid());
                    }
                    result.extend_from_slice(&[0; 4]);
                    continue;
                }
                let digit = if ascii {
                    byte.checked_sub(b'!')
                        .filter(|&v| v < 85)
                        .ok_or_else(invalid)?
                } else {
                    B85.iter().position(|&v| v == byte).ok_or_else(invalid)? as u8
                };
                group.push(digit);
                if group.len() == 5 {
                    let value = group.iter().fold(0u64, |n, &v| n * 85 + u64::from(v));
                    let value = u32::try_from(value).map_err(|_| invalid())?;
                    result.extend_from_slice(&value.to_be_bytes());
                    group.clear();
                }
            }
            if !group.is_empty() {
                let count = group.len();
                group.resize(5, 84);
                let value = group.iter().fold(0u64, |n, &v| n * 85 + u64::from(v));
                let value = u32::try_from(value).map_err(|_| invalid())?;
                result.extend_from_slice(&value.to_be_bytes()[..count - 1]);
            }
            Ok(result)
        }
        _ => Err(Error::Unsupported("base decoding")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_roundtrips_cover_partial_groups_and_every_byte() {
        for n in 0..260 {
            let data = (0..n).map(|i| (i * 173) as u8).collect::<Vec<_>>();
            for (enc, dec) in [
                ("b64encode", "b64decode"),
                ("urlsafe_b64encode", "urlsafe_b64decode"),
                ("b32encode", "b32decode"),
                ("b16encode", "b16decode"),
                ("a85encode", "a85decode"),
                ("b85encode", "b85decode"),
            ] {
                assert_eq!(
                    decode(dec, &encode(enc, &data, false).unwrap(), false, false).unwrap(),
                    data
                );
            }
        }
    }
}
