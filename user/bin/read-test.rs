#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let Ok(mut fd1) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("read_test: cannot open target.txt");
    };

    let mut buf = [0u8; 4096];

    // Expected: pseudo UE causes this read to fail.
    match fd1.read(&mut buf) {
        Ok(_) => {
            exit_with_msg("read_test: target read unexpectedly succeeded");
        }
        Err(_) => {
            // Expected recovery path.
        }
    }

    // Check whether another file can still be opened/read
    // after recovery.
    let Ok(mut fd2) = open("LICENSE", OpenFlag::READ_ONLY) else {
        exit_with_msg("read_test: cannot open LICENSE after recovery");
    };

    match fd2.read(&mut buf) {
        Ok(n) => {
            Stdout
                .write_all(&buf[..n])
                .expect("read_test: LICENSE write error");
        }
        Err(_) => {
            exit_with_msg("read_test: LICENSE read failed after recovery");
        }
    }

    println!("LICENSE read succeeded");

    // Check whether both descriptors can be closed without
    // panic/deadlock after recovery.
    if close(fd2).is_err() {
        exit_with_msg("read_test: cannot close fd2");
    }
    println!("fd2 close succeeded");

    if close(fd1).is_err() {
        exit_with_msg("read_test: cannot close recovered fd1");
    }

    println!("recovered fd1 close succeeded");
    println!("read recovery test passed");
}
