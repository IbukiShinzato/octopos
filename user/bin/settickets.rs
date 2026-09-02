#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 2 {
        exit_with_msg("usage: settickets tickets");
    }

    let tickets = args
        .get_str(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| {
            exit_with_msg("settickets: invalid tickets");
        });

    match settickets(tickets) {
        Ok(()) => {
            println!("success settickets: {tickets}");
        }
        Err(_) => {
            eprintln!("settickets: failed to settickets {tickets}");
            exit(1)
        }
    }
}
