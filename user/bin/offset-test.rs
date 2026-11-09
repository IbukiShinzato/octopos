#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let Ok(mut fd) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("offset-test: open failed");
    };

    let mut buf = [0u8; 4096];

    // 1回目：正常に5バイト読み込む
    match fd.read(&mut buf[..5]) {
        Ok(5) if &buf[..5] == b"recov" => {
            println!("Success: first read succeeded");
        }
        _ => exit_with_msg("offset-test: first read failed"),
    }

    // 2回目：疑似UEによってIoError
    match fd.read(&mut buf) {
        Err(SysError::IoError) => {
            println!("Success: UE injected");
        }
        _ => exit_with_msg("offset-test: expected IoError"),
    }

    // 3回目：offset=5から読み込みを再開
    match fd.read(&mut buf) {
        Ok(n) if &buf[..n] == b"ery test\n" => {
            println!("Success: offset restored");
        }
        _ => exit_with_msg("offset-test: recovery failed"),
    }

    if close(fd).is_err() {
        exit_with_msg("offset-test: close failed");
    }

    println!("TEST: offset recovery test passed");
}
