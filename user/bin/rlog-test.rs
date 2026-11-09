#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    // 1回目のopen
    let Ok(fd1) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("rlog-test: first open failed");
    };

    println!("TEST: first open succeeded");

    // 1回目のclose
    if close(fd1).is_err() {
        exit_with_msg("rlog-test: first close failed");
    }

    println!("TEST: first close succeeded");

    // 2回目のopen（File.idの再利用を確認）
    let Ok(fd2) = open("target.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("rlog-test: second open failed");
    };

    println!("TEST: second open succeeded");

    // 2回目のclose
    if close(fd2).is_err() {
        exit_with_msg("rlog-test: second close failed");
    }

    println!("TEST: second close succeeded");
    println!("TEST: rlog lifecycle test completed");
}
