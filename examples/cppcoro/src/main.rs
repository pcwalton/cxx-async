// cxx-async2/examples/cppcoro/src/main.rs

use async_recursion::async_recursion;
use cxx_async2::{define_cxx_future, CxxAsyncException};
use futures::executor::{self, ThreadPool};
use futures::join;
use futures::task::SpawnExt;
use once_cell::sync::Lazy;
use std::ops::Range;

#[cxx::bridge]
mod ffi {
    extern "Rust" {
        type RustFutureF64;
        type RustFutureString;
        fn rust_dot_product() -> Box<RustFutureF64>;
        fn rust_not_product() -> Box<RustFutureF64>;
        fn rust_cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;
    }

    unsafe extern "C++" {
        include!("cppcoro_example.h");

        fn cppcoro_dot_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_dot_product();
        fn cppcoro_schedule_rust_dot_product();
        fn cppcoro_not_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_not_product();
        fn cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;
    }
}

define_cxx_future!(F64, f64);
define_cxx_future!(String, String);

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

struct Xorshift {
    state: u32,
}

impl Xorshift {
    fn new() -> Xorshift {
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

fn main() {
    // Test Rust calling C++ async functions, both synchronously and via a scheduler.
    let future = ffi::cppcoro_dot_product();
    println!("{}", executor::block_on(future).unwrap());
    let future = ffi::cppcoro_dot_product();
    println!(
        "{}",
        executor::block_on(THREAD_POOL.spawn_with_handle(future).unwrap()).unwrap()
    );

    // Test C++ calling Rust async functions.
    ffi::cppcoro_call_rust_dot_product();
    ffi::cppcoro_schedule_rust_dot_product();

    // Test exceptions being thrown by C++ async functions.
    let future = ffi::cppcoro_not_product();
    match executor::block_on(future) {
        Ok(_) => panic!("shouldn't have succeeded!"),
        Err(err) => println!("{}", err.what()),
    }

    // Test errors being thrown by Rust async functions.
    ffi::cppcoro_call_rust_not_product();

    // Test yielding across the boundary repeatedly.
    let future = ffi::cppcoro_ping_pong(0);
    println!("{}", executor::block_on(future).unwrap());
}
