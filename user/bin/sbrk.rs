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

fn test_beyond_grow() {
    let base = sbrk(0x1000).expect("grow");
    unsafe {
        *((base + 0x1000) as *mut u8) = 0x42;
    } // should kill
    println!("FAIL: write beyond grow");
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

fn test_shrink_untouched() {
    let base = sbrk(0x1000).expect("grow");
    sbrk(-0x1000).expect("shrink");
    unsafe {
        *(base as *mut u8) = 0x42;
    } // should kill
    println!("FAIL: write beyond shrink");
}

fn test_shrink_touched() {
    let base = sbrk(0x1000).expect("grow");
    unsafe {
        *(base as *mut u8) = 0x42;
    }
    sbrk(-0x1000).expect("shrink");
    unsafe {
        *(base as *mut u8) = 0x42;
    } // should kill
    println!("FAIL: write beyond shrink");
}

fn test_multi_page_shrink() {
    let base = sbrk(4 * 0x1000).expect("grow 4 pages");
    unsafe {
        *((base + 3 * 0x1000) as *mut u8) = 0x42;
    }
    sbrk(-4 * 0x1000).expect("shrink 4 pages");
    unsafe {
        *((base + 2 * 0x1000) as *mut u8) = 0x42;
    } // should kill
    println!("FAIL: write beyond multi-page grow");
}

fn test_oom() {
    loop {
        let base = sbrk(0x1000).expect("grow");
        unsafe {
            *(base as *mut u8) = 0x42;
            assert_eq!(*(base as *mut u8), 0x42);
        }
    }
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_grow();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_beyond_grow();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_multi_page_grow();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_shrink_touched();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_shrink_untouched();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_multi_page_shrink();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    if fork().unwrap_or_else(|_| exit_with_msg("sbrk: fork failed")) == 0 {
        test_oom();
        exit(0)
    }
    wait(&mut 0).expect("sbrk: wait");

    println!("done");
}
