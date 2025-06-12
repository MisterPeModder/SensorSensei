/// The iterator type created by [`decode_form_url_encoded`].
pub struct DecodeFormUrlEncoded<'a> {
    data: &'a mut [u8],
}

/// Returns an iterator that yield key/value pairs for the given form-url-encoded-data.
///
/// Note: this *mutates* the buffer in-place to avoid allocations on the basis that
/// URL-encoded strings are always longer or have the same size as the decoded string.
///
/// See the unit tests for usage examples.
pub fn decode_form_url_encoded(data: &mut [u8]) -> DecodeFormUrlEncoded {
    DecodeFormUrlEncoded { data }
}

/// Performs in-place decoding
///
/// Note: this *mutates* the buffer in-place to avoid allocations on the basis that
/// URL-encoded strings are always longer or have the same size as the decoded string.
fn url_decode(data: &mut [u8]) -> &[u8] {
    let mut begin_offset = 0usize;
    let mut end_offset = data.len();

    while begin_offset < end_offset {
        (begin_offset, end_offset) = url_decode_next(data, begin_offset, end_offset);
    }

    &data[..end_offset]
}

fn url_decode_next(data: &mut [u8], begin_offset: usize, end_offset: usize) -> (usize, usize) {
    let next_special = memchr::memchr2(b'+', b'%', &data[begin_offset..end_offset]);

    if let Some(next_special) = next_special {
        let next_special = next_special + begin_offset;
        let special_char = data[next_special];

        if special_char == b'+' {
            // replace "+" by space
            data[next_special] = b' ';
        } else if next_special + 2 < end_offset {
            let digit1 = hex_digit_to_value(data[next_special + 1]);
            let digit2 = hex_digit_to_value(data[next_special + 2]);

            if let (Some(digit1), Some(digit2)) = (digit1, digit2) {
                // replace escape by actual byte
                data[next_special] = (digit1 << 4) | digit2;

                // shift the rest of the buffer two bytes to the left
                data[..end_offset].copy_within(next_special + 3.., next_special + 1);

                return (next_special + 1, end_offset - 2);
            }
        }

        (next_special + 1, end_offset)
    } else {
        (end_offset, end_offset)
    }
}

fn hex_digit_to_value(char: u8) -> Option<u8> {
    match char {
        b'0'..=b'9' => Some(char - b'0'),
        b'a'..=b'f' => Some(char - b'a' + 10),
        b'A'..=b'F' => Some(char - b'A' + 10),
        _ => None,
    }
}

impl<'a> Iterator for DecodeFormUrlEncoded<'a> {
    // (key, value) iterator
    type Item = (&'a [u8], &'a [u8]);

    fn next<'b>(&'b mut self) -> Option<Self::Item> {
        let mut data: &'a mut [u8] = &mut [];

        core::mem::swap(&mut data, &mut self.data);

        let kv_sep = memchr::memchr(b'=', data)?;
        let (raw_key, data): (&'a mut [u8], &'a mut [u8]) = data.split_at_mut(kv_sep);

        let kv_end = memchr::memchr(b'&', data).unwrap_or(data.len());
        let (raw_val, data): (&'a mut [u8], &'a mut [u8]) = data.split_at_mut(kv_end);

        let key: &'a [u8] = url_decode(raw_key);
        let val: &'a [u8] = url_decode(&mut raw_val[1..]);

        // Advance data to the next key-value pair if not at the end
        self.data = if data.is_empty() {
            &mut []
        } else {
            &mut data[1..]
        };

        Some((key, val))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_decode_form_url_encoded() {
        let mut encoded: Vec<u8> = br#"csrf_token=%7B%7B+csrf_token+%7D%7D&wifi_sta_ssid=external+ssid&wifi_sta_password=1234&wifi_ap_ssid=apSEE+D&dns_server_1=1.1.1.1&dns_server_2=1.0.0.1&action=apply"#.to_vec();
        let mut it = decode_form_url_encoded(&mut encoded);

        assert_eq!(
            it.next(),
            Some((b"csrf_token".as_ref(), b"{{ csrf_token }}".as_ref()))
        );
        assert_eq!(
            it.next(),
            Some((b"wifi_sta_ssid".as_ref(), b"external ssid".as_ref()))
        );
        assert_eq!(
            it.next(),
            Some((b"wifi_sta_password".as_ref(), b"1234".as_ref()))
        );
        assert_eq!(
            it.next(),
            Some((b"wifi_ap_ssid".as_ref(), b"apSEE D".as_ref()))
        );
        assert_eq!(
            it.next(),
            Some((b"dns_server_1".as_ref(), b"1.1.1.1".as_ref()))
        );
        assert_eq!(
            it.next(),
            Some((b"dns_server_2".as_ref(), b"1.0.0.1".as_ref()))
        );
        assert_eq!(it.next(), Some((b"action".as_ref(), b"apply".as_ref())));
        assert_eq!(it.next(), None);
    }

    #[test]
    fn test_url_decode_identity() {
        let mut encoded: Vec<u8> = br#""#.to_vec();
        assert_eq!(url_decode(&mut encoded), b"".as_ref());

        let mut encoded: Vec<u8> = b"should-not-change".to_vec();
        assert_eq!(url_decode(&mut encoded), b"should-not-change".as_ref());

        let mut encoded: Vec<u8> = b"%4nope%invalid".to_vec();
        assert_eq!(url_decode(&mut encoded), b"%4nope%invalid".as_ref());
    }

    #[test]
    fn test_url_decode_spaces() {
        let mut encoded: Vec<u8> = b"++".to_vec();
        assert_eq!(url_decode(&mut encoded), b"  ".as_ref());

        let mut encoded: Vec<u8> = b"[%20%20]".to_vec();
        assert_eq!(url_decode(&mut encoded), b"[  ]".as_ref());

        let mut encoded: Vec<u8> = b"%20s++".to_vec();
        assert_eq!(url_decode(&mut encoded), b" s  ".as_ref());

        let mut encoded: Vec<u8> = b"+++spa%20ce+++".to_vec();
        assert_eq!(url_decode(&mut encoded), b"   spa ce   ".as_ref());

        let mut encoded: Vec<u8> = b"++These+are%20spaces++".to_vec();
        assert_eq!(url_decode(&mut encoded), b"  These are spaces  ".as_ref());
    }

    #[test]
    fn test_url_decode_hex() {
        let mut encoded: Vec<u8> = br#"%7B%7b param %7d%7D"#.to_vec();
        assert_eq!(url_decode(&mut encoded), b"{{ param }}".as_ref());
    }

    #[test]
    fn test_url_decode_mixed() {
        let mut encoded: Vec<u8> = br#"csrf_token=%7B%7B+csrf_token+%7D%7D&wifi_sta_ssid=external+ssid&wifi_sta_password=1234&wifi_ap_ssid=apSEE+D&dns_server_1=1.1.1.1&dns_server_2=1.0.0.1&action=apply"#.to_vec();
        assert_eq!(url_decode(&mut encoded), b"csrf_token={{ csrf_token }}&wifi_sta_ssid=external ssid&wifi_sta_password=1234&wifi_ap_ssid=apSEE D&dns_server_1=1.1.1.1&dns_server_2=1.0.0.1&action=apply".as_ref());
    }
}
