// cxx-async/examples/cppcoro/src/main.rs
//
//! Demonstrates how to use `cxx-async` with `cppcoro`.

use async_recursion::async_recursion;
use crate::ffi::StringNamespaced;
use cxx_async::CxxAsyncException;
use futures::executor::{self, ThreadPool};
use futures::join;
use futures::task::SpawnExt;
use once_cell::sync::Lazy;
use std::ops::Range;

#[cxx::bridge]
mod ffi {
    #[derive(Debug, PartialEq)]
    struct StringNamespaced {
        namespaced_string: String,
    }

    extern "Rust" {
        type RustFutureF64;
        type RustFutureString;
        #[namespace = foo::bar]
        type RustFutureStringNamespaced;
        fn rust_dot_product() -> Box<RustFutureF64>;
        fn rust_not_product() -> Box<RustFutureF64>;
        fn rust_cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;
    }

    unsafe extern "C++" {
        include!("cppcoro_example.h");

        fn cppcoro_dot_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_dot_product() -> f64;
        fn cppcoro_schedule_rust_dot_product() -> f64;
        fn cppcoro_get_namespaced_string() -> Box<RustFutureStringNamespaced>;
        fn cppcoro_not_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_not_product() -> String;
        fn cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;
    }
}

#[cxx_async::bridge_future]
struct RustFutureF64(f64);
#[cxx_async::bridge_future]
struct RustFutureString(String);
#[cxx_async::bridge_future(namespace = foo::bar)]
struct RustFutureStringNamespaced(StringNamespaced);

const VECTOR_LENGTH: usize = 16384;
const SPLIT_LIMIT: usize = 32;

static THREAD_POOL: Lazy<ThreadPool> = Lazy::new(|| ThreadPool::new().unwrap());

static VECTORS: Lazy<(Vec<f64>, Vec<f64>)> = Lazy::new(|| {
    let mut rand = Xorshift::new();
    let (mut vector_a, mut vector_b) = (vec![], vec![]);
    for _ in 0..VECTOR_LENGTH {
        vector_a.push(rand.next() as f64);
        vector_b.push(rand.next() as f64);
    }
    (vector_a, vector_b)
});

// Simple PRNG that can be easily duplicated on the Rust and C++ sides to ensure identical output.
struct Xorshift {
    state: u32,
}

impl Xorshift {
    fn new() -> Xorshift {
        // Random, but constant, seed.
        Xorshift { state: 0x243f6a88 }
    }

    fn next(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }
}

#[async_recursion]
async fn dot_product(range: Range<usize>) -> f64 {
    let len = range.end - range.start;
    if len > SPLIT_LIMIT {
        let mid = (range.start + range.end) / 2;
        let (first, second) = join!(
            THREAD_POOL
                .spawn_with_handle(dot_product(range.start..mid))
                .unwrap(),
            dot_product(mid..range.end)
        );
        return first + second;
    }

    let (ref a, ref b) = *VECTORS;
    let sum = range.clone().map(|index| a[index] * b[index]).sum();
    sum
}

fn rust_dot_product() -> Box<RustFutureF64> {
    RustFutureF64::infallible(dot_product(0..VECTOR_LENGTH))
}

fn rust_not_product() -> Box<RustFutureF64> {
    RustFutureF64::fallible(async {
        Err(CxxAsyncException::new("kapow".to_owned().into_boxed_str()))
    })
}

fn rust_cppcoro_ping_pong(i: i32) -> Box<RustFutureString> {
    RustFutureString::infallible(async move {
        format!(
            "{}ping ",
            if i < 4 {
                ffi::cppcoro_ping_pong(i + 1).await.unwrap()
            } else {
                "".to_owned()
            }
        )
    })
}

// Tests Rust calling C++ synchronously.
#[test]
fn test_rust_calling_cpp_synchronously() {
    assert_eq!(
        executor::block_on(ffi::cppcoro_dot_product()).unwrap(),
        75719554055754070000000.0
    );
    assert_eq!(
        executor::block_on(ffi::cppcoro_get_namespaced_string()).unwrap(),
        StringNamespaced {
            namespaced_string: "hello world".to_owned(),
        }
    );
}

// Tests Rust calling C++ on a scheduler.
#[test]
fn test_rust_calling_cpp_on_scheduler() {
    let future = ffi::cppcoro_dot_product();
    let value = executor::block_on(THREAD_POOL.spawn_with_handle(future).unwrap()).unwrap();
    assert_eq!(value, 75719554055754070000000.0);
}

// Tests C++ calling async Rust code synchronously.
#[test]
fn test_cpp_calling_rust_synchronously() {
    assert_eq!(
        ffi::cppcoro_call_rust_dot_product(),
        75719554055754070000000.0
    );
}

// Tests C++ calling async Rust code on a scheduler.
#[test]
fn test_cpp_calling_rust_on_scheduler() {
    assert_eq!(
        ffi::cppcoro_schedule_rust_dot_product(),
        75719554055754070000000.0
    );
}

// Tests Rust calling async C++ code throwing exceptions.
#[test]
fn test_cpp_async_functions_throwing_exceptions() {
    match executor::block_on(ffi::cppcoro_not_product()) {
        Ok(_) => panic!("shouldn't have succeeded"),
        Err(err) => assert_eq!(err.what(), "kaboom"),
    }
}

// Tests C++ calling async Rust code returning errors.
#[test]
fn test_rust_async_functions_returning_errors() {
    assert_eq!(ffi::cppcoro_call_rust_not_product(), "kapow");
}

// Tests sending values across the language barrier synchronously.
#[test]
fn test_ping_pong() {
    let result = executor::block_on(ffi::cppcoro_ping_pong(0)).unwrap();
    assert_eq!(result, "ping pong ping pong ping pong ping pong ping pong ");
}

fn main() {
    // Test Rust calling C++ async functions, both synchronously and via a scheduler.
    let future = ffi::cppcoro_dot_product();
    println!("{}", executor::block_on(future).unwrap());
    let future = ffi::cppcoro_dot_product();
    println!(
        "{}",
        executor::block_on(THREAD_POOL.spawn_with_handle(future).unwrap()).unwrap()
    );
    let future = ffi::cppcoro_get_namespaced_string();
    println!("{:?}", executor::block_on(future).unwrap());

    // Test C++ calling Rust async functions.
    println!("{}", ffi::cppcoro_call_rust_dot_product());
    println!("{}", ffi::cppcoro_schedule_rust_dot_product());

    // Test exceptions being thrown by C++ async functions.
    let future = ffi::cppcoro_not_product();
    match executor::block_on(future) {
        Ok(_) => panic!("shouldn't have succeeded!"),
        Err(err) => println!("{}", err.what()),
    }

    // Test errors being thrown by Rust async functions.
    println!("{}", ffi::cppcoro_call_rust_not_product());

    // Test yielding across the boundary repeatedly.
    let future = ffi::cppcoro_ping_pong(0);
    println!("{}", executor::block_on(future).unwrap());
}
