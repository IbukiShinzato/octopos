#![no_std]
#![no_main]

use user::*;

const NPROC: usize = 64;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 1 {
        exit_with_msg("usage: ps");
    }

    let mut pstats = [PStat::default(); NPROC];

    match getpinfo(&mut pstats) {
        Ok(()) => {
            println!("PID\tINUSE\tTICKETS\tPASS\t\tSTRIDE\t\tN_SCHEDULE");

            for pstat in pstats.iter().filter(|pstat| pstat.inuse != 0) {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t\t{}",
                    pstat.pid,
                    pstat.inuse,
                    pstat.tickets,
                    pstat.pass,
                    pstat.stride,
                    pstat.n_schedule
                );
            }
        }
        Err(e) => {
            eprintln!("ps: failed: {}", e);
            exit(1);
        }
    }
}
