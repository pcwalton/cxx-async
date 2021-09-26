// cxx-async2/examples/folly/src/main.rs

use crate::ffi::{RustOneshotF64, RustOneshotString};
use async_recursion::async_recursion;
use cxx_async2::{define_cxx_future, CxxAsyncException};
use ffi::RustExecletBundleF64;
use futures::executor::{self, ThreadPool};
use futures::join;
use futures::task::SpawnExt;
use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::future::Future;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[cxx::bridge]
mod ffi {
    // Boilerplate for F64
    pub struct RustOneshotF64 {
        pub future: Box<RustFutureF64>,
        pub sender: Box<RustSenderF64>,
    }
    pub struct RustExecletBundleF64 {
        pub future: Box<RustFutureF64>,
        pub execlet: Box<RustExecletF64>,
    }
    extern "Rust" {
        type RustFutureF64;
        type RustSenderF64;
        type RustExecletF64;
        // FIXME(pcwalton): Maybe collapse these into a single function?
        unsafe fn channel(self: &RustFutureF64, value: *const f64) -> RustOneshotF64;
        unsafe fn execlet(self: &RustFutureF64) -> RustExecletBundleF64;
        unsafe fn send(self: &mut RustSenderF64, status: u32, value: *const u8);
        unsafe fn poll(self: &mut RustFutureF64, result: *mut u8, waker_data: *const u8) -> u32;
        unsafe fn submit(self: &RustExecletF64, task: *mut u8);
        unsafe fn send(self: &RustExecletF64, value: *const f64);
    }

    // FIXME(pcwalton): Move these?
    #[namespace = "cxx::async"]
    unsafe extern "C++" {
        include!("cxx_async_folly.h");

        unsafe fn execlet_run_task(execlet: *const u8, task: *mut u8);
    }

    // Boilerplate for String
    pub struct RustOneshotString {
        pub future: Box<RustFutureString>,
        pub sender: Box<RustSenderString>,
    }
    extern "Rust" {
        type RustFutureString;
        type RustSenderString;
        unsafe fn channel(self: &RustFutureString, value: *const String) -> RustOneshotString;
        unsafe fn send(self: &mut RustSenderString, status: u32, value: *const u8);
        unsafe fn poll(self: &mut RustFutureString, result: *mut u8, waker_data: *const u8) -> u32;
    }

    extern "Rust" {
        fn rust_dot_product() -> Box<RustFutureF64>;
        fn rust_not_product() -> Box<RustFutureF64>;
        //fn rust_folly_ping_pong(i: i32) -> Box<RustFutureString>;
    }

    unsafe extern "C++" {
        include!("folly_example.h");

        fn folly_dot_product() -> Box<RustFutureF64>;
        fn folly_call_rust_dot_product();
        fn folly_schedule_rust_dot_product();
        fn folly_not_product() -> Box<RustFutureF64>;
        fn folly_call_rust_not_product();
        //fn folly_ping_pong(i: i32) -> Box<RustFutureString>;
    }
}

#[derive(Clone)]
pub struct RustExecletF64(Arc<Mutex<RustExecletDataF64>>);

struct RustExecletTaskF64(*mut u8);

struct RustExecletDataF64 {
    runqueue: VecDeque<RustExecletTaskF64>,
    result: Option<f64>,
    waker: Option<Waker>,
    running: bool,
}

unsafe impl Send for RustExecletDataF64 {}
unsafe impl Sync for RustExecletDataF64 {}

struct RustExecletFutureF64 {
    execlet: RustExecletF64,
}

impl RustExecletF64 {
    fn new() -> RustExecletF64 {
        RustExecletF64(Arc::new(Mutex::new(RustExecletDataF64 {
            runqueue: VecDeque::new(),
            result: None,
            waker: None,
            running: false,
        })))
    }

    unsafe fn submit(&self, task: *mut u8) {
        //println!("submit()");
        let mut this = self.0.lock().unwrap();
        this.runqueue.push_back(RustExecletTaskF64(task));
        if let Some(ref waker) = this.waker {
            // Avoid possible deadlocks.
            // FIXME(pcwalton): Is this necessary?
            let waker = (*waker).clone();
            drop(this);

            //println!("asking waker to wake us up...");
            waker.wake_by_ref();
        } else {
            //println!("no waker to wake...")
        }
    }

    unsafe fn send(&self, value: *const f64) {
        let mut this = self.0.lock().unwrap();
        assert!(this.result.is_none());
        let mut staging = MaybeUninit::uninit();
        ptr::copy_nonoverlapping(value, staging.as_mut_ptr(), 1);
        this.result = Some(staging.assume_init());

        // Don't do this if we're running, or we might end up in a situation where the waker tries
        // to poll us again, which is UB (and will deadlock in the C++ bindings).
        if !this.running {
            this.waker
                .as_ref()
                .expect("Send with no waker present?")
                .wake_by_ref();
        }
    }
}

impl RustFutureF64 {
    unsafe fn execlet(_: &RustFutureF64) -> RustExecletBundleF64 {
        let execlet = RustExecletF64::new();
        let future = RustFutureF64::from(RustExecletFutureF64 {
            execlet: execlet.clone(),
        });
        RustExecletBundleF64 {
            future,
            execlet: Box::new(execlet),
        }
    }
}

impl Future for RustExecletFutureF64 {
    type Output = f64;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        //println!("poll()");
        let mut guard = self.execlet.0.lock().unwrap();
        debug_assert!(!guard.running);
        guard.running = true;

        //println!("poll() grabbed lock");
        guard.waker = Some((*cx.waker()).clone());
        while let Some(task) = guard.runqueue.pop_front() {
            drop(guard);
            //println!("poll() running task");
            unsafe {
                ffi::execlet_run_task(&self.execlet as *const RustExecletF64 as *const u8, task.0);
            }
            //println!("poll() ran task, grabbing lock again");
            guard = self.execlet.0.lock().unwrap();
        }

        guard.running = false;
        match guard.result.take() {
            Some(result) => {
                //println!("no tasks left, got result and returning it!");
                Poll::Ready(result)
            }
            None => {
                //println!("no tasks left! returning pending");
                Poll::Pending
            }
        }
    }
}

// Application code follows

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
    RustFutureF64::from(dot_product(0..VECTOR_LENGTH))
}

fn rust_not_product() -> Box<RustFutureF64> {
    async fn go() -> Result<f64, CxxAsyncException> {
        Err(CxxAsyncException::new("kapow".to_owned().into_boxed_str()))
    }
    RustFutureF64::from_fallible(go())
}

/*
fn rust_folly_ping_pong(i: i32) -> Box<RustFutureString> {
    async fn go(i: i32) -> String {
        format!(
            "{}ping ",
            if i < 4 {
                ffi::folly_ping_pong(i + 1).await.unwrap()
            } else {
                "".to_owned()
            }
        )
    }
    RustFutureString::from(go(i))
}
*/

fn main() {
    // Test Rust calling C++ async functions, both synchronously and via a scheduler.
    let future = ffi::folly_dot_product();
    println!("{}", executor::block_on(future).unwrap());
    let future = ffi::folly_dot_product();
    println!(
        "{}",
        executor::block_on(THREAD_POOL.spawn_with_handle(future).unwrap()).unwrap()
    );

    // Test C++ calling Rust async functions.
    ffi::folly_call_rust_dot_product();
    ffi::folly_schedule_rust_dot_product();

    // Test exceptions being thrown by C++ async functions.
    let future = ffi::folly_not_product();
    match executor::block_on(future) {
        Ok(_) => panic!("shouldn't have succeeded!"),
        Err(err) => println!("{}", err.what()),
    }

    // Test errors being thrown by Rust async functions.
    ffi::folly_call_rust_not_product();

    /*
    // Test yielding across the boundary repeatedly.
    let future = ffi::folly_ping_pong(0);
    println!("{}", executor::block_on(future).unwrap());
    */
}
