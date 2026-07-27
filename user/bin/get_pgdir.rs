#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    match get_pgdir() {
        Ok(pa) => println!("pa: 0x{pa:x}"),
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}
