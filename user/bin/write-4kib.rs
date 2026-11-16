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

    let buf = [b'a'; 4096];
    let mut written = 0;

    while written < buf.len() {
        match fd.write(&buf[written..]) {
            Ok(0) => panic!("write: zero bytes written"),
            Ok(n) => written += n,
            Err(e) => panic!("write failed: {:?}", e),
        }
    }

    assert_eq!(written, 4096);

    let buf2 = [b'!'; 1];
    written = 0;

    while written < buf2.len() {
        match fd.write(&buf2[written..]) {
            Ok(0) => panic!("write: zero bytes written"),
            Ok(n) => written += n,
            Err(e) => panic!("write failed: {:?}", e),
        }
    }
    close(fd).expect("close");

    let mut fd = open("target.txt", OpenFlag::READ_ONLY).expect("open for read");

    let mut msg = [0u8; 4097];
    let mut total_read = 0;

    while total_read < msg.len() {
        match fd.read(&mut msg[total_read..]) {
            Ok(0) => panic!("unexpected EOF"),
            Ok(n) => total_read += n,
            Err(e) => panic!("read failed: {:?}", e),
        }
    }

    assert_eq!(total_read, 4097);
    assert_eq!(&msg[..4096], &buf[..]);
    assert_eq!(&msg[4096..], &buf2[..]);

    close(fd).expect("close read");

    println!("TEST: 4KiB write test passed");
}
