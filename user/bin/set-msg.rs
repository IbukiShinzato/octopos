#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 3 {
        exit_with_msg("usage: set_msg message len");
    }

    let buf = args.get_str(1).unwrap_or_else(|| {
        exit_with_msg("set_msg: invalid message");
    });

    let len = args
        .get_str(2)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| {
            exit_with_msg("set_msg: invalid length");
        });

    match set_msg(buf.as_bytes(), len) {
        Ok(n) => println!("set_msg: stored {} bytes", n),
        Err(e) => eprintln!("set_msg: failed: {}", e),
    }
}
