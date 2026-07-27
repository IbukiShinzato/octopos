#![no_std]
#![no_main]

use user::*;

fn test_normal_message() {
    let messages = ["hello", "octopos", "test", "message"];
    let mut buf = [0u8; 64];

    for message in messages.iter().map(|s| s.as_bytes()) {
        let len = message.len();

        let set_len = set_msg(message, len).expect("set_msg failed");
        assert_eq!(set_len, len);

        let get_len = get_msg(&mut buf, len).expect("get_msg failed");
        assert_eq!(get_len, len);
        assert_eq!(&buf[..get_len], message);
    }
}

fn test_partial_message() {
    let message = b"octopos";
    let mut buf = [0u8; 64];

    let set_len = set_msg(message, 4).expect("partial set_msg failed");
    assert_eq!(set_len, 4);

    let get_len = get_msg(&mut buf, 4).expect("partial get_msg failed");
    assert_eq!(get_len, 4);
    assert_eq!(&buf[..get_len], b"octo");
}

fn test_overwrite_message() {
    let mut buf = [0u8; 64];

    let first = b"long message";
    let second = b"short";

    assert_eq!(
        set_msg(first, first.len()).expect("first set_msg failed"),
        first.len()
    );

    assert_eq!(
        set_msg(second, second.len()).expect("second set_msg failed"),
        second.len()
    );

    let get_len = get_msg(&mut buf, second.len()).expect("get_msg failed");

    assert_eq!(get_len, second.len());
    assert_eq!(&buf[..get_len], second);

    assert!(get_msg(&mut buf, first.len()).is_err());
}

fn test_invalid_set_length() {
    let message = b"test";

    assert!(set_msg(message, 10).is_err());
}

fn test_invalid_get_length() {
    let message = b"test";
    let mut buf = [0u8; 64];

    set_msg(message, message.len()).expect("set_msg failed");

    assert!(get_msg(&mut buf, 5).is_err());
}

fn test_empty_message() {
    let mut buf = [0u8; 1];

    let set_len = set_msg(b"", 0).expect("empty set_msg failed");
    assert_eq!(set_len, 0);

    let get_len = get_msg(&mut buf, 0).expect("empty get_msg failed");
    assert_eq!(get_len, 0);
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_normal_message();
    test_partial_message();
    test_overwrite_message();
    test_invalid_set_length();
    test_invalid_get_length();
    test_empty_message();
}
