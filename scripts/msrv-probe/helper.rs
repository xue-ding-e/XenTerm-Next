// Equivalent fallback from the parked compatibility candidate, not a product change.
fn boundary(text: &str, index: usize) -> usize {
    let mut boundary = index.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

#[test]
fn ascii_boundaries() {
    for index in 0..=3 {
        assert_eq!(boundary("abc", index), index);
    }
}

#[test]
fn multibyte_boundaries_and_trigger_offset() {
    let text = "aé中💻z";
    let expected = [0, 1, 3, 3, 6, 6, 6, 10, 10, 10, 10, 11];
    for (index, expected) in expected.into_iter().enumerate() {
        assert_eq!(boundary(text, index), expected);
        assert!(text.is_char_boundary(expected));
    }
    let text = "中".repeat(100);
    // Actual trigger offset: 300 bytes - keep 256 = byte 44, rounded to 45.
    let split = boundary(&text, text.len() - 256);
    assert_eq!(split, 45);
    assert_eq!(&text[split..], "中".repeat(85));
}

#[test]
fn out_of_range_and_empty() {
    for text in ["", "abc", "é中💻"] {
        for index in [text.len(), text.len() + 1, usize::MAX] {
            assert_eq!(boundary(text, index), text.len());
        }
    }
}
