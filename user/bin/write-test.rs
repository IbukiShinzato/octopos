#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let mut fd = open(
        "target.txt",
        OpenFlag::WRITE_ONLY | OpenFlag::CREATE | OpenFlag::TRUNCATE,
    )
    .expect("open");

    // 1回目
    println!("DEBUG: first write");
    let mut buf = b"hello";
    let mut written = 0;
    while written < buf.len() {
        match fd.write(&buf[written..]) {
            Ok(0) => exit_with_msg("write: zero bytes written"),
            Ok(n) => {
                written += n;
            }
            Err(_) => {
                exit_with_msg("write: write error");
            }
        }
    }

    // 2回目
    println!("DEBUG: second write");
    buf = b"world";
    written = 0;
    while written < buf.len() {
        match fd.write(&buf[written..]) {
            Ok(0) => exit_with_msg("write: zero bytes written"),
            Ok(n) => {
                written += n;
            }
            Err(_) => {
                exit_with_msg("write: write error");
            }
        }
    }

    // 3回目
    println!("DEBUG: third write");
    let buf = b"!";
    written = 0;
    while written < buf.len() {
        match fd.write(&buf[written..]) {
            Ok(0) => exit_with_msg("write: zero bytes written"),
            Ok(n) => {
                written += n;
            }
            Err(_) => {
                exit_with_msg("write: write error");
            }
        }
    }

    close(fd).expect("close");
    let mut fd = open("target.txt", OpenFlag::READ_ONLY).expect("open");

    println!("DEBUG: first read");
    let mut msg = [0u8; 512];
    match fd.read(&mut msg) {
        Ok(n) => {
            assert_eq!(n, 11, "unexpected read size");
            assert_eq!(&msg[..n], b"helloworld!", "content mismatch");

            println!("TEST: success write test!");
        }

        Err(e) => {
            panic!("write-test: read failed: {:?}", e);
        }
    };

    close(fd).expect("write: close failed");
}
