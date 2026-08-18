#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    match pwd() {
        Ok(pathname) => println!("{pathname}"),
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}
