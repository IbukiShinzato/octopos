#![no_std]
#![no_main]

use user::*;

const TEST_BUFSIZE: usize = 512;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 2 {
        exit_with_msg("usage: get_msg len");
    }

    let len = args
        .get_str(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| {
            exit_with_msg("get_msg: invalid length");
        });

    if len > TEST_BUFSIZE {
        exit_with_msg("get_msg: length exceeds test buffer size");
    }

    let mut buf = [0u8; TEST_BUFSIZE];

    match get_msg(&mut buf[..len], len) {
        Ok(n) => {
            print!("get_msg: ");
            Stdout
                .write_all(&buf[..n])
                .unwrap_or_else(|_| exit_with_msg("get_msg: write failed"));
            println!();
            println!("get_msg: received {} bytes", n);
        }
        Err(e) => {
            eprintln!("get_msg: failed: {}", e);
        }
    }
}
