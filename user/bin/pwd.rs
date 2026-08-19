#![no_std]
#![no_main]

use user::*;

const BUFSIZE: usize = 4096;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    let mut buf = [0u8; BUFSIZE];

    match pwd(&mut buf) {
        Ok(n) => {
            let path = str::from_utf8(&buf[..n]).unwrap();
            println!("{path}");
        }
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}
