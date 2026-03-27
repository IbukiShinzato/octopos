#![no_std]
#![no_main]

use user::*;

const TESTS: &[&str] = &["/cow", "/sbrk"];

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("running {} tests\n", TESTS.len());

    let mut passed = 0;
    let mut failed = 0;

    for name in TESTS {
        print!("test {} ... ", &name[1..]);

        let pid = fork().unwrap_or_else(|_| exit_with_msg("testrunner: fork failed"));
        if pid == 0 {
            exec(name, &[&name[1..]]);
            exit(1);
        }

        let mut code = 0;
        wait(&mut code).expect("testrunner: wait failed");

        if code == 0 {
            println!("ok");
            passed += 1;
        } else {
            println!("FAILED");
            failed += 1;
        }
    }

    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        passed,
        failed,
    );

    poweroff(if failed == 0 { 0 } else { 1 });
}
