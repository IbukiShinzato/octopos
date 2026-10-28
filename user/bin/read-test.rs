#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let Ok(mut fd1) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("read_test: cannot open target.txt");
    };

    let mut buf = [0u8; 4096];
    // 1回目；擬似UEによってIoError想定
    match fd1.read(&mut buf) {
        Err(SysError::IoError) => {
            println!("Success: first read failed with IoError");
        }
        Err(_) => {
            exit_with_msg("read_test: unexpected first read error");
        }
        Ok(_) => {
            exit_with_msg("read_test: first read unexpectedly succeeded");
        }
    }

    // 2回目：リカバリによってOk(n)想定
    match fd1.read(&mut buf) {
        Ok(n) => {
            if &buf[..n] != b"recovery test\n" {
                exit_with_msg("read_test: recovered content mismatch");
            }

            println!("Success: second read succeeded");
        }
        Err(_) => {
            exit_with_msg("read_test: second read failed");
        }
    }

    if close(fd1).is_err() {
        exit_with_msg("read_test: close failed");
    }

    println!("TEST: read recovery test passed");
}
