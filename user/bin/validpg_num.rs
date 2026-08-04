#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    match get_validpg_num() {
        Ok(pg_count) => println!("pg_count: {pg_count}"),
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}
