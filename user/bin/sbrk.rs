#![no_std]
#![no_main]

use user::*;

fn test_grow() {
    let base = sbrk(0x1000).expect("grow");
    unsafe {
        *(base as *mut u8) = 0x42;
        assert_eq!(*(base as *mut u8), 0x42);
    }
}

/// Writing one byte past the grown region should kill the process.
fn test_beyond_grow() {
    let base = sbrk(0x1000).expect("grow");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        unsafe { *((base + 0x1000) as *mut u8) = 0x42 };
        println!("FAIL: write beyond grow");
        exit(1);
    }
    wait(&mut 0).expect("sbrk: wait");
}

fn test_multi_page_grow() {
    let base = sbrk(4 * 0x1000).expect("grow 4 pages");
    for i in 0..4 {
        unsafe {
            *((base + i * 0x1000) as *mut u8) = i as u8;
            assert_eq!(*((base + i * 0x1000) as *mut u8), i as u8);
        }
    }
}

/// Shrinking a page that was never touched, then writing to it, should kill the process.
fn test_shrink_untouched() {
    let base = sbrk(0x1000).expect("grow");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        sbrk(-0x1000).expect("shrink");
        unsafe { *(base as *mut u8) = 0x42 };
        println!("FAIL: write beyond shrink");
        exit(1);
    }
    wait(&mut 0).expect("sbrk: wait");
}

/// Shrinking a page that was previously written to, then writing to it again, should kill the process.
fn test_shrink_touched() {
    let base = sbrk(0x1000).expect("grow");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        unsafe { *(base as *mut u8) = 0x42 };
        sbrk(-0x1000).expect("shrink");
        unsafe { *(base as *mut u8) = 0x42 };
        println!("FAIL: write beyond shrink");
        exit(1);
    }
    wait(&mut 0).expect("sbrk: wait");
}

/// After shrinking multiple pages, writing into any of the removed pages should kill the process.
fn test_multi_page_shrink() {
    let base = sbrk(4 * 0x1000).expect("grow 4 pages");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        unsafe { *((base + 3 * 0x1000) as *mut u8) = 0x42 };
        sbrk(-4 * 0x1000).expect("shrink 4 pages");
        unsafe { *((base + 2 * 0x1000) as *mut u8) = 0x42 };
        println!("FAIL: write beyond multi-page shrink");
        exit(1);
    }
    wait(&mut 0).expect("sbrk: wait");
}

/// Growing indefinitely should eventually exhaust memory and kill the process.
fn test_oom() {
    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        loop {
            let base = sbrk(0x1000).expect("grow");
            unsafe {
                *(base as *mut u8) = 0x42;
                assert_eq!(*(base as *mut u8), 0x42);
            }
        }
    }
    wait(&mut 0).expect("sbrk: wait");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_grow();
    test_beyond_grow();
    test_multi_page_grow();
    test_shrink_touched();
    test_shrink_untouched();
    test_multi_page_shrink();
    test_oom();
}
