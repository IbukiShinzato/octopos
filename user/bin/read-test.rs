#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let Ok(mut fd1) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("read_test: cannot open target.txt");
    };

    // １回目：通常動作
    let mut buf = [0u8; 512];
    match fd1.read(&mut buf[..8]) {
        Ok(n) => {
            assert_eq!(n, 8);
            assert_eq!(&buf[..n], b"recovery", "read-test: content mismatch");
        }
        Err(_) => {
            exit_with_msg("read_test: unexpected first read error");
        }
    }

    // ２回目：故障注入が有効なら透過的Restoringを検証
    buf = [0u8; 512];
    match fd1.read(&mut buf) {
        Ok(n) => {
            assert_eq!(n, 6);
            assert_eq!(&buf[..n], b" test\n", "read-test: content mismatch");
        }
        Err(_) => {
            exit_with_msg("read_test: second read failed");
        }
    }

    // ３回目：EOFを確認
    let mut buf = [0u8; 512];
    match fd1.read(&mut buf) {
        Ok(n) => {
            assert_eq!(n, 0, "read-test: unexpected data after recovery");
        }
        Err(e) => {
            panic!("read-test: third read failed: {:?}", e);
        }
    }

    if close(fd1).is_err() {
        exit_with_msg("read_test: close failed");
    }

    println!("TEST: read recovery test passed");
}
