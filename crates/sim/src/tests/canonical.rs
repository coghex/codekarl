use std::collections::BTreeMap;

use crate::{Canonical, CanonicalError};

fn round_trip<T: Canonical + PartialEq + std::fmt::Debug>(value: &T, bytes: &[u8]) {
    assert_eq!(value.to_bytes().unwrap(), bytes);
    assert_eq!(&T::from_bytes(bytes).unwrap(), value);
}

#[test]
fn integers_are_little_endian_at_their_declared_width() {
    round_trip(&0x12_u8, &[0x12]);
    round_trip(&0x1234_u16, &[0x34, 0x12]);
    round_trip(&0x1234_5678_u32, &[0x78, 0x56, 0x34, 0x12]);
    round_trip(&0x0102_0304_0506_0708_u64, &[8, 7, 6, 5, 4, 3, 2, 1]);
    round_trip(&-2_i8, &[0xfe]);
    round_trip(&-2_i16, &[0xfe, 0xff]);
    round_trip(&-2_i32, &[0xfe, 0xff, 0xff, 0xff]);
    round_trip(&i64::MIN, &[0, 0, 0, 0, 0, 0, 0, 0x80]);
}

#[test]
fn bool_is_one_byte() {
    round_trip(&false, &[0]);
    round_trip(&true, &[1]);
}

#[test]
fn strings_and_byte_strings_carry_a_u32_length() {
    round_trip(&String::new(), &[0, 0, 0, 0]);
    round_trip(&"é".to_owned(), &[2, 0, 0, 0, 0xc3, 0xa9]);
    round_trip(&vec![9_u8, 8], &[2, 0, 0, 0, 9, 8]);
}

#[test]
fn sequences_carry_a_u32_count_and_options_a_tag() {
    round_trip(&vec![1_u16, 2], &[2, 0, 0, 0, 1, 0, 2, 0]);
    round_trip(&Vec::<u64>::new(), &[0, 0, 0, 0]);
    round_trip(&None::<u8>, &[0]);
    round_trip(&Some(7_u8), &[1, 7]);
}

#[test]
fn maps_are_written_in_key_order_whatever_the_insertion_order() {
    let mut map = BTreeMap::new();
    map.insert("b".to_owned(), 2_u8);
    map.insert("a".to_owned(), 1_u8);
    round_trip(
        &map,
        &[2, 0, 0, 0, 1, 0, 0, 0, b'a', 1, 1, 0, 0, 0, b'b', 2],
    );
}

#[test]
fn a_short_input_is_refused_at_every_cut() {
    let bytes = vec!["ab".to_owned(), "c".to_owned()].to_bytes().unwrap();
    for cut in 0..bytes.len() {
        assert!(
            matches!(
                Vec::<String>::from_bytes(&bytes[..cut]),
                Err(CanonicalError::ShortInput { .. })
            ),
            "cut at {cut}"
        );
    }
    assert_eq!(
        u32::from_bytes(&[1, 2]),
        Err(CanonicalError::ShortInput {
            needed: 2,
            available: 2
        })
    );
}

#[test]
fn a_huge_count_with_no_elements_is_a_short_input_not_an_allocation() {
    assert!(matches!(
        Vec::<u64>::from_bytes(&[0xff, 0xff, 0xff, 0xff]),
        Err(CanonicalError::ShortInput { .. })
    ));
}

#[test]
fn a_bad_tag_is_refused() {
    assert_eq!(
        Option::<u8>::from_bytes(&[2, 0]),
        Err(CanonicalError::BadTag {
            ty: "Option",
            tag: 2
        })
    );
}

#[test]
fn a_bool_other_than_0_or_1_is_refused() {
    assert_eq!(
        bool::from_bytes(&[2]),
        Err(CanonicalError::BadBool { value: 2 })
    );
}

#[test]
fn invalid_utf8_is_refused() {
    assert_eq!(
        String::from_bytes(&[2, 0, 0, 0, 0xc3, 0x28]),
        Err(CanonicalError::InvalidUtf8)
    );
}

#[test]
fn trailing_bytes_are_refused() {
    assert_eq!(
        u16::from_bytes(&[1, 0, 9]),
        Err(CanonicalError::TrailingBytes { count: 1 })
    );
}

#[test]
fn map_keys_out_of_order_or_repeated_are_refused() {
    // Keys 2 then 1, and keys 1 then 1.
    let entry = |key: u8| [key, 0];
    for keys in [[2, 1], [1, 1]] {
        let mut bytes = vec![2, 0, 0, 0];
        for key in keys {
            bytes.extend_from_slice(&entry(key));
        }
        assert_eq!(
            BTreeMap::<u8, u8>::from_bytes(&bytes),
            Err(CanonicalError::KeysNotAscending),
            "{keys:?}"
        );
    }
}

#[test]
fn re_encoding_a_decoded_value_gives_the_same_bytes() {
    let mut map = BTreeMap::new();
    for key in ["x", "yy", "zzz"] {
        map.insert(key.to_owned(), Some(vec![u8::try_from(key.len()).unwrap()]));
    }
    let bytes = map.to_bytes().unwrap();
    let decoded = BTreeMap::<String, Option<Vec<u8>>>::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.to_bytes().unwrap(), bytes);
}
