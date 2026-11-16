#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 3 {
        exit_with_msg("usage: write file message")
    }

    let mut fd = args
        .get_str(1)
        .and_then(|path| {
            open(
                path,
                OpenFlag::READ_WRITE | OpenFlag::CREATE | OpenFlag::TRUNCATE,
            )
            .ok()
        })
        .expect("write: invalid path");

    let buf = args.get(2).expect("write: invalid buf");

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

    close(fd).expect("write: close failed");
}
