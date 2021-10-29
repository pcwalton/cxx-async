// cxx-async/src/main.rs
//
//! `cxx-async` is a Rust crate that extends the [`cxx`](http://cxx.rs/) library to provide
//! seamless interoperability between asynchronous Rust code using `async`/`await` and [C++20
//! coroutines] using `co_await`. If your C++ code is asynchronous, `cxx-async` can provide a more
//! convenient, and potentially more efficient, alternative to callbacks. You can freely convert
//! between C++ coroutines and Rust futures and await one from the other.
//!
//! It's important to emphasize what `cxx-async` isn't: it isn't a C++ binding to Tokio or any
//! other Rust I/O library. Nor is it a Rust binding to `boost::asio` or similar. Such bindings
//! could in principle be layered on top of `cxx-async` if desired, but this crate doesn't provide
//! them out of the box. (Note that this is a tricky problem even in theory, since Rust async I/O
//! code is generally tightly coupled to a single library such as Tokio, in much the same way C++
//! async I/O code tends to be tightly coupled to libraries like `boost::asio`.) If you're writing
//! server code, you can still use `cxx-async`, but you will need to ensure that both the Rust and
//! C++ sides run separate I/O executors.
//!
//! `cxx-async` aims for compatibility with popular C++ coroutine support libraries. Right now,
//! both the lightweight [`cppcoro`](https://github.com/lewissbaker/cppcoro) and the more
//! comprehensive [Folly](https://github.com/facebook/folly/) are supported. Pull requests are
//! welcome to support others.
//!
//! ## Quick tutorial
//!
//! To use `cxx-async`, first start by adding `cxx` to your project. Then add the following to your
//! `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! cxx-async = "0.1"
//! ```
//!
//! Now, inside your `#[cxx::bridge]` module, declare a future type and some methods like so:
//!
//! ```ignore
//! #[cxx::bridge]
//! mod ffi {
//!     // Give each future type that you want to bridge a name.
//!     extern "Rust" {
//!         type RustFutureString;
//!     }
//!
//!     // Async C++ methods that you wish Rust to call go here. Make sure they return one of the
//!     // boxed future types you declared above.
//!     unsafe extern "C++" {
//!         fn hello_from_cpp() -> Box<RustFutureString>;
//!     }
//!
//!     // Async Rust methods that you wish C++ to call go here. Again, make sure they return one of
//!     // the boxed future types you declared above.
//!     extern "Rust" {
//!         fn hello_from_rust() -> Box<RustFutureString>;
//!     }
//! }
//! ```
//!
//! After the `#[cxx::bridge]` block, define the future types using the
//! `#[cxx_async::bridge_future]` attribute:
//!
//! ```
//! // The inner type is the Rust type that this future yields.
//! #[cxx_async::bridge_future]
//! struct RustFutureString(String);
//! ```
//!
//! Now, in your C++ file, make sure to `#include` the right headers:
//!
//! ```cpp
//! #include "rust/cxx.h"
//! #include "rust/cxx_async.h"
//! #include "rust/cxx_async_cppcoro.h"  // Or cxx_async_folly.h, as appropriate.
//! ```
//!
//! And add a call to the `CXXASYNC_DEFINE_FUTURE` macro to define the C++ side of the future:
//!
//! ```cpp
//! // The first argument is the name you gave the future, and the second argument is the
//! // corresponding C++ type. The latter is the C++ type that `cxx` maps your Rust type to: in this
//! // case, `String` maps to `rust::String`, so we supply `rust::String` here.
//! CXXASYNC_DEFINE_FUTURE(RustFutureString, rust::String);
//! ```
//!
//! You're all set! Now you can define asynchronous C++ code that Rust can call:
//!
//! ```cpp
//! rust::Box<RustFutureString> hello_from_cpp() {
//!     co_return std::string("Hello world!");
//! }
//! ```
//!
//! On the Rust side:
//!
//! ```ignore
//! async fn call_cpp() -> String {
//!     // This returns a Result (with the error variant populated if C++ threw an exception), so
//!     // you need to unwrap it:
//!     ffi::hello_from_cpp().await.unwrap()
//! }
//! ```
//!
//! And likewise, define some asynchronous Rust code that C++ can call:
//!
//! ```ignore
//! use cxx_async::CxxAsyncResult;
//! fn hello_from_rust() -> Box<RustFutureString> {
//!     // You can instead use `fallible` if your async block returns a Result.
//!     RustFutureString::infallible(async { "Hello world!".to_owned() })
//! }
//! ```
//!
//! Over on the C++ side:
//!
//! ```cpp
//! cppcoro::task<rust::String> call_rust() {
//!     co_return hello_from_rust();
//! }
//! ```
//!
//! That's it! You should now be able to freely await futures on either side.
//!
//! [C++20 coroutines]: https://en.cppreference.com/w/cpp/language/coroutines

#![warn(missing_docs)]

extern crate link_cplusplus;

use futures::channel::oneshot::{
    self, Canceled, Receiver as OneshotReceiver, Sender as OneshotSender,
};
use std::collections::VecDeque;
use std::convert::From;
use std::error::Error;
use std::ffi::CStr;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

const FUTURE_STATUS_PENDING: u32 = 0;
const FUTURE_STATUS_COMPLETE: u32 = 1;
const FUTURE_STATUS_ERROR: u32 = 2;

pub use cxx_async_macro::bridge_future;

#[doc(hidden)]
pub use pin_utils::unsafe_pinned;

// Bridged glue functions.
extern "C" {
    fn cxxasync_suspended_coroutine_clone(waker_data: *mut u8) -> *mut u8;
    fn cxxasync_suspended_coroutine_wake(waker_data: *mut u8);
    fn cxxasync_suspended_coroutine_wake_by_ref(waker_data: *mut u8);
    fn cxxasync_suspended_coroutine_drop(waker_data: *mut u8);
}

// A suspended C++ coroutine needs to act as a waker if it awaits a Rust future. This vtable
// provides that glue.
static CXXASYNC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    rust_suspended_coroutine_clone,
    rust_suspended_coroutine_wake,
    rust_suspended_coroutine_wake_by_ref,
    rust_suspended_coroutine_drop,
);

/// Any exception that a C++ coroutine throws is automatically caught and converted into this error
/// type.
///
/// This is just a wrapper around the result of `std::exception::what()`.
#[derive(Debug)]
pub struct CxxAsyncException {
    what: Box<str>,
}

impl CxxAsyncException {
    /// Creates a new exception with the given error message.
    pub fn new(what: Box<str>) -> Self {
        Self { what }
    }

    /// The value returned by `std::exception::what()`.
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

/// A convenient shorthand for `Result<T, CxxAsyncException>`.
pub type CxxAsyncResult<T> = Result<T, CxxAsyncException>;

// A table of functions that the `define_cxx_future!` macro emits for the C++ bridge to use.
//
// This must match the definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncVtable {
    pub channel: *mut u8,
    pub sender_send: *mut u8,
    pub future_poll: *mut u8,
}

unsafe impl Send for CxxAsyncVtable {}
unsafe impl Sync for CxxAsyncVtable {}

// A sender/receiver pair for the return value of a wrapped C++ coroutine.
//
// This is an implementation detail and is not exposed to the programmer. It must match the
// definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncOneshot<Fut, Out>
where
    Fut: Future<Output = CxxAsyncResult<Out>>,
{
    // The receiving end.
    future: Box<Fut>,
    // The sending end.
    sender: Box<CxxAsyncSender<Out>>,
}

// Allows the Rust polling interface to drive C++ tasks to completion.
//
// This is needed by the Folly backend, to allow awaiting semifutures.
#[derive(Clone)]
#[doc(hidden)]
pub struct Execlet(Arc<RustExeclet>);

// The type that C++ sees for an execlet. This is opaque as far as C++ is concerned.
#[doc(hidden)]
pub struct RustExeclet(Mutex<ExecletImpl>);

impl Execlet {
    // Creates a new execlet with no waker and an empty runqueue.
    fn new() -> Execlet {
        Execlet(Arc::new(RustExeclet(Mutex::new(ExecletImpl {
            runqueue: VecDeque::new(),
            waker: None,
            running: false,
        }))))
    }

    // Consumes the reference.
    unsafe fn from_raw(ptr: *const RustExeclet) -> Execlet {
        Execlet(Arc::from_raw(ptr))
    }

    // Bumps the reference count.
    unsafe fn from_raw_ref(ptr: *const RustExeclet) -> Execlet {
        let this = Execlet::from_raw(ptr);
        mem::forget(this.clone());
        this
    }

    // Runs all tasks in the runqueue to completion.
    fn run(&self, cx: &mut Context) {
        // Lock.
        let mut guard = self.0.0.lock().unwrap();
        debug_assert!(!guard.running);
        guard.running = true;

        // Run as many tasks as we have.
        guard.waker = Some((*cx.waker()).clone());
        while let Some(task) = guard.runqueue.pop_front() {
            // Drop the lock so that the task that we run can safely enqueue new tasks without
            // deadlocking.
            drop(guard);
            unsafe {
                task.run();
            }
            // Re-acquire the lock.
            guard = self.0.0.lock().unwrap();
        }

        // Unlock.
        guard.running = false;
    }

    // Submits a task to this execlet.
    fn submit(&self, task: ExecletTask) {
        let mut this = self.0.0.lock().unwrap();
        this.runqueue.push_back(task);
        if !this.running {
            if let Some(ref waker) = this.waker {
                // Avoid possible deadlocks.
                let waker = (*waker).clone();
                drop(this);
                waker.wake_by_ref();
            }
        }
    }
}

struct ExecletImpl {
    // Tasks waiting to run.
    runqueue: VecDeque<ExecletTask>,
    // A Rust Waker that will be informed when we need to wake up and run some tasks in our
    // runqueue or yield the result.
    waker: Option<Waker>,
    // True if we're running; false otherwise. This flag is necessary to avoid deadlocks resulting
    // from recursive invocations.
    running: bool,
}

// The concrete type of the future that wraps a C++ coroutine.
//
// The programmer only interacts with this abstractly behind a `Box<dyn Future>` trait object, so
// this type is considered an implementation detail. It must be public because the
// `define_cxx_future!` macro needs to name it.
#[doc(hidden)]
pub struct CxxAsyncReceiver<Output> {
    receiver: OneshotReceiver<CxxAsyncResult<Output>>,
    execlet: Option<Execlet>,
}

// The concrete type of the sending end of a future.
//
// This must be public because the `define_cxx_future!` macro needs to name it.
#[doc(hidden)]
pub struct CxxAsyncSender<Output>(Option<OneshotSender<CxxAsyncResult<Output>>>);

impl<Output> Future for CxxAsyncReceiver<Output> {
    type Output = CxxAsyncResult<Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(ref execlet) = self.execlet {
            execlet.run(cx);
        }
        match Future::poll(Pin::new(&mut self.receiver), cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            Poll::Ready(Err(Canceled)) => Poll::Ready(Err(CxxAsyncException::new(
                "Canceled".to_owned().into_boxed_str(),
            ))),
        }
    }
}

impl<Output> From<OneshotReceiver<CxxAsyncResult<Output>>> for CxxAsyncReceiver<Output> {
    fn from(receiver: OneshotReceiver<CxxAsyncResult<Output>>) -> Self {
        CxxAsyncReceiver {
            receiver,
            execlet: None,
        }
    }
}

// The sending end that the C++ bridge uses to return a value to a Rust future.
//
// This is an implementation detail.
#[doc(hidden)]
pub trait RustSender {
    type Output;
    fn send(&mut self, value: CxxAsyncResult<Self::Output>);
}

// A continuation in an execlet's run queue.
struct ExecletTask {
    // A C++ stub that resumes this task.
    run: unsafe extern "C" fn(*mut u8),
    // The task data, passed to `run`.
    data: *mut u8,
}

impl ExecletTask {
    // Creates a new `ExecletTask`.
    fn new(run: unsafe extern "C" fn(*mut u8), data: *mut u8) -> Self {
        Self { run, data }
    }

    // Resumes the task.
    unsafe fn run(self) {
        (self.run)(self.data)
    }
}

unsafe impl Send for ExecletTask {}
unsafe impl Sync for ExecletTask {}

/// Wraps an arbitrary Rust Future in a boxed `cxx-async` future so that it can be returned to C++.
///
/// You should not need to implement this manually; it's automatically implemented by the
/// `define_cxx_future!` macro.
pub trait IntoCxxAsyncFuture {
    /// The type of the value yielded by the future.
    type Output;

    /// Wraps a Rust Future that directly returns the output type.
    ///
    /// Use this when you aren't interested in propagating errors to C++ as exceptions.
    fn infallible<Fut>(future: Fut) -> Box<Self>
    where
        Fut: Future<Output = Self::Output> + Send + 'static,
    {
        Self::fallible(async move { Ok(future.await) })
    }

    /// Wraps a Rust Future that returns the output type, wrapped in a `CxxAsyncResult`.
    ///
    /// Use this when you have error values that you want to turn into exceptions on the C++ side.
    fn fallible<Fut>(future: Fut) -> Box<Self>
    where
        Fut: Future<Output = CxxAsyncResult<Self::Output>> + Send + 'static;
}

// Creates a new oneshot sender/receiver pair.
//
// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
#[doc(hidden)]
pub unsafe extern "C" fn channel<Fut, Out>(
    out_oneshot: *mut CxxAsyncOneshot<Fut, Out>,
    execlet: *mut RustExeclet,
) where
    Fut: From<CxxAsyncReceiver<Out>> + Future<Output = CxxAsyncResult<Out>>,
{
    let (sender, receiver) = oneshot::channel();
    let oneshot = CxxAsyncOneshot {
        sender: Box::new(CxxAsyncSender(Some(sender))),
        future: Box::new(
            CxxAsyncReceiver {
                receiver,
                execlet: Some(Execlet::from_raw_ref(execlet)),
            }
            .into(),
        ),
    };
    ptr::copy_nonoverlapping(&oneshot, out_oneshot, 1);
    mem::forget(oneshot);
}

unsafe fn unpack_value_to_send<Output>(status: u32, value: *const u8) -> CxxAsyncResult<Output> {
    match status {
        FUTURE_STATUS_COMPLETE => {
            let mut staging: MaybeUninit<Output> = MaybeUninit::uninit();
            ptr::copy_nonoverlapping(value as *const Output, staging.as_mut_ptr(), 1);
            Ok(staging.assume_init())
        }
        FUTURE_STATUS_ERROR => {
            let string = CStr::from_ptr(value as *const i8);
            Err(CxxAsyncException::new(
                string.to_string_lossy().into_owned().into_boxed_str(),
            ))
        }
        _ => unreachable!(),
    }
}

// C++ calls this to yield a final value.
//
// SAFETY: This is a low-level function called by our C++ code.
//
// Takes ownership of the value. The caller must not call its destructor.
//
// Any errors when sending are dropped on the floor. This is the right behavior because futures
// can be legally dropped in Rust to signal cancellation.
#[doc(hidden)]
pub unsafe extern "C" fn sender_send<Output>(
    this: &mut CxxAsyncSender<Output>,
    status: u32,
    value: *const u8,
) {
    drop(
        this.0
            .take()
            .unwrap()
            .send(unpack_value_to_send(status, value)),
    );
}

// C++ calls this to poll a wrapped Rust future.
//
// SAFETY:
// * This is a low-level function called by our C++ code.
// * `Pin<&mut Future>` is marked `#[repr(transparent)]`, so it's FFI-safe.
#[doc(hidden)]
pub unsafe extern "C" fn future_poll<Fut, Out>(
    this: Pin<&mut Fut>,
    result: *mut u8,
    waker_data: *const u8,
) -> u32
where
    Fut: Future<Output = CxxAsyncResult<Out>>,
{
    let waker = Waker::from_raw(RawWaker::new(
        waker_data as *const (),
        &CXXASYNC_WAKER_VTABLE,
    ));
    let mut context = Context::from_waker(&waker);
    match this.poll(&mut context) {
        Poll::Ready(Ok(value)) => {
            ptr::copy_nonoverlapping(&value, result as *mut Out, 1);
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

// The C++ bridge indirectly calls this to drop types.
//
// SAFETY: This is a low-level function called (indirectly) by `cxx`'s C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn drop_glue<T>(ptr: *mut Box<T>) {
    let mut boxed: MaybeUninit<Box<T>> = MaybeUninit::uninit();
    ptr::copy_nonoverlapping(ptr, boxed.as_mut_ptr(), 1);
    drop(boxed.assume_init());
}

// Bumps the reference count on a suspended C++ coroutine.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_clone(address: *const ()) -> RawWaker {
    RawWaker::new(
        cxxasync_suspended_coroutine_clone(address as *mut () as *mut u8) as *mut () as *const (),
        &CXXASYNC_WAKER_VTABLE,
    )
}

// Resumes a suspended C++ coroutine and decrements its reference count.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake(address: *const ()) {
    cxxasync_suspended_coroutine_wake(address as *mut () as *mut u8)
}

// Resumes a suspended C++ coroutine without decrementing its reference count.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake_by_ref(address: *const ()) {
    cxxasync_suspended_coroutine_wake_by_ref(address as *mut () as *mut u8)
}

// Decrements the reference count on a suspended C++ coroutine.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_drop(address: *const ()) {
    cxxasync_suspended_coroutine_drop(address as *mut () as *mut u8)
}

// Execlet FFI

// C++ calls this to create a new execlet.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_create() -> *const RustExeclet {
    Arc::into_raw(Execlet::new().0)
}

// C++ calls this to decrement the reference count on an execlet and free it if the count hits zero.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_release(this: *mut RustExeclet) {
    drop(Execlet::from_raw(this))
}

// C++ calls this to submit a task to the execlet. This internally bumps the reference count.
#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn cxxasync_execlet_submit(
    this: *mut RustExeclet,
    run: extern "C" fn(*mut u8),
    task_data: *mut u8,
) {
    Execlet::from_raw_ref(this).submit(ExecletTask::new(run, task_data))
}
