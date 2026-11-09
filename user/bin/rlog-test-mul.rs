#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let Ok(mut fd1) = open("target1.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("rlog-test-mul: first file open failed");
    };

    let Ok(mut fd2) = open("target2.txt", OpenFlag::READ_ONLY) else {
        exit_with_msg("rlog-test-mul: second file open failed");
    };

    let mut buf = [0u8; 4096];

    // File 1: 5バイト正常に読み込む
    match fd1.read(&mut buf[..5]) {
        Ok(5) if &buf[..5] == b"recov" => {
            println!("Success: File 1 first read succeeded");
        }
        _ => exit_with_msg("rlog-test-mul: File 1 first read failed"),
    }

    // File 2: 5バイト正常に読み込む
    match fd2.read(&mut buf[..5]) {
        Ok(5) if &buf[..5] == b"recov" => {
            println!("Success: File 2 first read succeeded");
        }
        _ => exit_with_msg("rlog-test-mul: File 2 first read failed"),
    }

    // File 1: 2回目のreadで疑似UEを発生させる
    match fd1.read(&mut buf) {
        Err(SysError::IoError) => {
            println!("Success: File 1 UE injected");
        }
        _ => exit_with_msg("rlog-test-mul: expected IoError"),
    }

    // File 1: Restoring後、offset=5から読み込みを再開
    match fd1.read(&mut buf) {
        Ok(n) if &buf[..n] == b"ery test1\n" => {
            println!("Success: File 1 recovery succeeded");
        }
        _ => exit_with_msg("rlog-test-mul: File 1 recovery failed"),
    }

    // File 2: 故障の影響を受けずoffset=5から読み込めるか確認
    match fd2.read(&mut buf) {
        Ok(n) if &buf[..n] == b"ery test2\n" => {
            println!("Success: File 2 unaffected");
        }
        _ => exit_with_msg("rlog-test-mul: File 2 read failed"),
    }

    close(fd1).expect("close File 1");
    close(fd2).expect("close File 2");

    println!("TEST: multiple file recovery test passed");
}
