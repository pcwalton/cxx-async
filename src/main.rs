// cxx-async2/src/main.rs

extern crate link_cplusplus;

use crate::ffi::{
    suspended_coroutine_clone, suspended_coroutine_drop, suspended_coroutine_wake,
    suspended_coroutine_wake_by_ref, RustOneshotF64, RustOneshotString,
};
use async_recursion::async_recursion;
use futures::channel::oneshot::{self, Canceled, Receiver, Sender};
use futures::executor;
use futures::future::BoxFuture;
use futures::join;
use once_cell::sync::Lazy;
use paste::paste;
use pin_utils::unsafe_pinned;
use std::error::Error;
use std::ffi::CStr;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::pin::Pin;
use std::ptr;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

const FUTURE_STATUS_PENDING: u32 = 0;
const FUTURE_STATUS_COMPLETE: u32 = 1;
const FUTURE_STATUS_ERROR: u32 = 2;

#[cxx::bridge]
mod ffi {
    // Boilerplate for F64
    pub struct RustOneshotF64 {
        pub future: Box<RustFutureF64>,
        pub sender: Box<RustSenderF64>,
    }
    extern "Rust" {
        type RustFutureF64;
        type RustSenderF64;
        unsafe fn channel(self: &RustFutureF64, value: *const f64) -> RustOneshotF64;
        unsafe fn send(self: &mut RustSenderF64, status: u32, value: *const u8);
        unsafe fn poll(self: &mut RustFutureF64, result: *mut u8, waker_data: *const u8) -> u32;
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

    // General

    #[namespace = "cxx::async"]
    unsafe extern "C++" {
        unsafe fn suspended_coroutine_clone(waker_data: *mut u8) -> *mut u8;
        unsafe fn suspended_coroutine_wake(waker_data: *mut u8);
        unsafe fn suspended_coroutine_wake_by_ref(waker_data: *mut u8);
        unsafe fn suspended_coroutine_drop(waker_data: *mut u8);
    }

    // Application

    extern "Rust" {
        fn rust_dot_product() -> Box<RustFutureF64>;
        fn rust_not_product() -> Box<RustFutureF64>;
        fn rust_cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;
    }

    unsafe extern "C++" {
        include!("cxx_async_waker.h");
        include!("cppcoro_example.h");
        //include!("folly_example.h");

        fn cppcoro_dot_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_dot_product();
        fn cppcoro_not_product() -> Box<RustFutureF64>;
        fn cppcoro_call_rust_not_product();
        fn cppcoro_ping_pong(i: i32) -> Box<RustFutureString>;

        //fn folly_call_rust_dot_product();
    }
}

static CXXASYNC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    rust_suspended_coroutine_clone,
    rust_suspended_coroutine_wake,
    rust_suspended_coroutine_wake_by_ref,
    rust_suspended_coroutine_drop,
);

#[derive(Debug)]
pub struct CxxAsyncException {
    what: Box<str>,
}

impl CxxAsyncException {
    pub fn new(what: Box<str>) -> Self {
        Self { what }
    }

    pub fn what(&self) -> &str {
        &self.what
    }
}

impl Display for CxxAsyncException {
    fn fmt(&self, formatter: &mut Formatter) -> FmtResult {
        formatter.write_str(&self.what)
    }
}

impl Error for CxxAsyncException {}

pub type CxxAsyncResult<T> = Result<T, CxxAsyncException>;

macro_rules! define_cxx_future {
    ($name:ident, $output:ty) => {
        paste! {
            pub struct [<RustFuture $name>] {
                future: BoxFuture<'static, CxxAsyncResult<$output>>,
            }

            pub struct [<RustSender $name>](Option<Sender<CxxAsyncResult<$output>>>);

            struct [<RustReceiver $name>](Receiver<CxxAsyncResult<$output>>);

            impl [<RustFuture $name>] {
                // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
                // (1) The struct does not implement Drop (other than for debugging, which doesn't
                // move the field).
                // (2) The struct doesn't implement Unpin.
                // (3) The struct isn't `repr(packed)`.
                unsafe_pinned!(future: BoxFuture<'static, CxxAsyncResult<$output>>);

                fn from(future: impl Future<Output = $output> + Send + 'static) -> Box<Self> {
                    return Self::from_fallible(wrapper(future));

                    async fn wrapper(
                        future: impl Future<Output = $output> + Send + 'static,
                    ) -> CxxAsyncResult<$output> {
                        Ok(future.await)
                    }
                }

                fn from_fallible(
                    future: impl Future<Output = CxxAsyncResult<$output>> + Send + 'static,
                ) -> Box<Self> {
                    Box::new([<RustFuture $name>] {
                        future: Box::pin(future),
                    })
                }

                unsafe fn channel(&self, _: *const $output) -> [<RustOneshot $name>] {
                    let (sender, receiver) = oneshot::channel();
                    [<RustOneshot $name>] {
                        sender: Box::new([<RustSender $name>](Some(sender))),
                        future: Box::new([<RustFuture $name>] {
                            future: Box::pin([<RustReceiver $name>](receiver)),
                        }),
                    }
                }

                unsafe fn poll(&mut self, result: *mut u8, waker_data: *const u8) -> u32 {
                    let waker = Waker::from_raw(RawWaker::new(
                        waker_data as *const (),
                        &CXXASYNC_WAKER_VTABLE,
                    ));
                    let mut context = Context::from_waker(&waker);
                    match Pin::new(&mut self.future).poll(&mut context) {
                        Poll::Ready(Ok(value)) => {
                            ptr::copy_nonoverlapping(&value, result as *mut $output, 1);
                            mem::forget(value);
                            FUTURE_STATUS_COMPLETE
                        }
                        Poll::Ready(Err(error)) => {
                            let error = error.what().to_owned();
                            ptr::copy_nonoverlapping(&error, result as *mut String, 1);
                            mem::forget(error);
                            FUTURE_STATUS_ERROR
                        }
                        Poll::Pending => FUTURE_STATUS_PENDING,
                    }
                }
            }

            impl [<RustSender $name>] {
                // Takes ownership of the value. The caller must not call its destructor.
                unsafe fn send(&mut self, status: u32, value: *const u8) {
                    let to_send = match status {
                        FUTURE_STATUS_COMPLETE => {
                            let mut staging: MaybeUninit<$output> = MaybeUninit::uninit();
                            ptr::copy_nonoverlapping(
                                value as *const $output,
                                staging.as_mut_ptr(),
                                1);
                            Ok(staging.assume_init())
                        }
                        FUTURE_STATUS_ERROR => {
                            let string = CStr::from_ptr(value as *const i8);
                            Err(CxxAsyncException::new(
                                string.to_string_lossy().into_owned().into_boxed_str(),
                            ))
                        }
                        _ => unreachable!(),
                    };

                    self.0.take().unwrap().send(to_send).unwrap();
                }
            }

            /*
            impl Drop for [<RustFuture $name>] {
                fn drop(&mut self) {
                    println!("destroying future");
                }
            }
            */

            impl Future for [<RustFuture $name>] {
                type Output = CxxAsyncResult<$output>;
                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    self.future().poll(cx)
                }
            }

            impl Future for [<RustReceiver $name>] {
                type Output = CxxAsyncResult<$output>;
                fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    match Pin::new(&mut self.0).poll(cx) {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(Ok(value)) => Poll::Ready(value),
                        Poll::Ready(Err(Canceled)) => Poll::Ready(Err(CxxAsyncException::new(
                            "Canceled".to_owned().into_boxed_str(),
                        ))),
                    }
                }
            }
        }
    };
}

unsafe fn rust_suspended_coroutine_clone(address: *const ()) -> RawWaker {
    RawWaker::new(
        suspended_coroutine_clone(address as *mut () as *mut u8) as *mut () as *const (),
        &CXXASYNC_WAKER_VTABLE,
    )
}

unsafe fn rust_suspended_coroutine_wake(address: *const ()) {
    suspended_coroutine_wake(address as *mut () as *mut u8)
}

unsafe fn rust_suspended_coroutine_wake_by_ref(address: *const ()) {
    suspended_coroutine_wake_by_ref(address as *mut () as *mut u8)
}

unsafe fn rust_suspended_coroutine_drop(address: *const ()) {
    suspended_coroutine_drop(address as *mut () as *mut u8)
}

// Application code follows:

define_cxx_future!(F64, f64);
define_cxx_future!(String, String);

const SPLIT_LIMIT: usize = 32;

static VECTORS: Lazy<(Vec<f64>, Vec<f64>)> = Lazy::new(|| {
    let mut rand = Xorshift::new();
    let (mut vector_a, mut vector_b) = (vec![], vec![]);
    for _ in 0..16384 {
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
async fn dot_product(a: &[f64], b: &[f64]) -> f64 {
    if a.len() > SPLIT_LIMIT {
        let half_count = a.len() / 2;
        let (first, second) = join!(
            dot_product(&a[0..half_count], &b[0..half_count]),
            dot_product(&a[half_count..], &b[half_count..])
        );
        return first + second;
    }

    let mut sum = 0.0;
    for (&a, &b) in a.iter().zip(b.iter()) {
        sum += a * b;
    }
    sum
}

fn rust_dot_product() -> Box<RustFutureF64> {
    let (ref vector_a, ref vector_b) = *VECTORS;
    RustFutureF64::from(dot_product(&vector_a, &vector_b))
}

fn rust_not_product() -> Box<RustFutureF64> {
    async fn go() -> Result<f64, CxxAsyncException> {
        Err(CxxAsyncException::new("kapow".to_owned().into_boxed_str()))
    }
    RustFutureF64::from_fallible(go())
}

fn rust_cppcoro_ping_pong(i: i32) -> Box<RustFutureString> {
    async fn go(i: i32) -> String {
        format!(
            "{}ping ",
            if i < 4 {
                ffi::cppcoro_ping_pong(i + 1).await.unwrap()
            } else {
                "".to_owned()
            }
        )
    }
    RustFutureString::from(go(i))
}

fn test_cppcoro() {
    let future = ffi::cppcoro_dot_product();
    println!("{}", executor::block_on(future).unwrap());

    // Test C++ calling Rust async functions.
    ffi::cppcoro_call_rust_dot_product();

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

/*
fn test_folly() {
    ffi::folly_call_rust_dot_product();
}
*/

fn main() {
    test_cppcoro();
    //test_folly();
}
