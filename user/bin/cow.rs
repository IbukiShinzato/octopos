#![no_std]
#![no_main]

use user::*;

/// Verify parent and child see independent copies of the same page after a write.
fn test_basic_cow() {
    let mut val: u32 = 42;

    if fork().unwrap_or_else(|_| exit_with_msg("cow: fork failed")) == 0 {
        val = 99;
        assert_eq!(val, 99);
        exit(0);
    }

    wait(&mut 0).expect("cow: wait");
    // parent's copy must be unchanged
    assert_eq!(val, 42, "FAIL: parent saw child's write");
}

/// Fork a chain of children, each modifying the same variable.
/// Every generation must see its own value, not a sibling's.
fn test_fork_chain() {
    let mut val = 0;

    for i in 0..4 {
        if fork().unwrap_or_else(|_| exit_with_msg("cow: fork failed")) == 0 {
            val = i;
            assert_eq!(val, i);
            exit(0);
        }
        wait(&mut 0).expect("cow: wait");
    }

    assert_eq!(val, 0, "FAIL: parent's val was modified");
}

/// Write a full page of data before forking, then verify child gets the right
/// snapshot and parent is not affected by child's overwrite.
fn test_full_page() {
    let base = sbrk(0x1000).expect("grow");
    let page = base as *mut u8;

    for i in 0..0x1000 {
        unsafe { *page.add(i) = (i & 0xFF) as u8 };
    }

    if fork().unwrap_or_else(|_| exit_with_msg("cow: fork failed")) == 0 {
        // overwrite entire page in child
        for i in 0..0x1000 {
            unsafe { *page.add(i) = 0xFF };
        }
        exit(0);
    }

    wait(&mut 0).expect("cow: wait");

    // parent's page must still hold the original pattern
    for i in 0..0x1000 {
        assert_eq!(
            unsafe { *page.add(i) },
            (i & 0xFF) as u8,
            "FAIL: parent page corrupted at offset {}",
            i
        );
    }
}

/// Verify that writing to a page in the child process triggers lazy allocation, and does not affect
/// the parent process's page.
fn test_cow_lazy() {
    let base = sbrk(0x1000).expect("grow");

    if fork().unwrap_or_else(|_| exit_with_msg("cow: fork failed")) == 0 {
        // write to the page in child, which should trigger lazy allocation
        unsafe { *(base as *mut u8) = 42 };
        assert_eq!(
            unsafe { *(base as *mut u8) },
            42,
            "FAIL: child page should have been lazily allocated and modified"
        );
        exit(0);
    }

    wait(&mut 0).expect("cow: wait");

    // parent's page should still be zero-initialized
    assert_eq!(
        unsafe { *(base as *mut u8) },
        0,
        "FAIL: parent page should be zero-initialized"
    );

}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_basic_cow();
    test_fork_chain();
    test_full_page();
    test_cow_lazy();
}
