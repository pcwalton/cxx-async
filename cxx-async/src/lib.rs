/*
 * Copyright (c) Meta Platforms, Inc. and its affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

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
//!     // Declare type aliases for each of the future types you wish to use here. Then declare
//!     // async C++ methods that you wish Rust to call. Make sure they return one of the future
//!     // types you declared.
//!     unsafe extern "C++" {
//!         type RustFutureString = crate::RustFutureString;
//!
//!         fn hello_from_cpp() -> RustFutureString;
//!     }
//!
//!     // Async Rust methods that you wish C++ to call go here. Again, make sure they return one of
//!     // the future types you declared above.
//!     extern "Rust" {
//!         fn hello_from_rust() -> RustFutureString;
//!     }
//! }
//! ```
//!
//! After the `#[cxx::bridge]` block, define the future types using the
//! `#[cxx_async::bridge]` attribute:
//!
//! ```
//! // The `Output` type is the Rust type that this future yields.
//! #[cxx_async::bridge]
//! unsafe impl Future for RustFutureString {
//!     type Output = String;
//! }
//! ```
//!
//! Now, in your C++ header, make sure to `#include` the right headers:
//!
//! ```cpp
//! #include "rust/cxx.h"
//! #include "rust/cxx_async.h"
//! #include "rust/cxx_async_cppcoro.h"  // Or cxx_async_folly.h, as appropriate.
//! ```
//!
//! And add a call to the `CXXASYNC_DEFINE_FUTURE` macro in your headers to define the C++ side of
//! the future:
//!
//! ```cpp
//! // The first argument is the name you gave the future, and the second argument is the
//! // corresponding C++ type, with `::` namespace separators replaced with commas. The latter is
//! // the C++ type that `cxx` maps your Rust type to: in this case, `String` maps to
//! // `rust::String`, so we supply `rust, String` here.
//! CXXASYNC_DEFINE_FUTURE(RustFutureString, rust, String);
//! ```
//!
//! You're all set! Now you can define asynchronous C++ code that Rust can call:
//!
//! ```cpp
//! RustFutureString hello_from_cpp() {
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
//! fn hello_from_rust() -> RustFutureString {
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

use futures::{Stream, StreamExt};
use once_cell::sync::OnceCell;
use std::collections::VecDeque;
use std::convert::From;
use std::error::Error;
use std::ffi::CStr;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::{ptr, thread};

const FUTURE_STATUS_PENDING: u32 = 0;
const FUTURE_STATUS_COMPLETE: u32 = 1;
const FUTURE_STATUS_ERROR: u32 = 2;
const FUTURE_STATUS_RUNNING: u32 = 3;

const SEND_RESULT_WAIT: u32 = 0;
const SEND_RESULT_SENT: u32 = 1;
const SEND_RESULT_FINISHED: u32 = 2;

pub use cxx_async_macro::bridge;

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

// A table of functions that the `bridge` macro emits for the C++ bridge to use.
//
// This must match the definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncVtable {
    pub channel: *mut u8,
    pub sender_send: *mut u8,
    pub sender_drop: *mut u8,
    pub future_poll: *mut u8,
    pub future_drop: *mut u8,
}

unsafe impl Send for CxxAsyncVtable {}
unsafe impl Sync for CxxAsyncVtable {}

// A sender/receiver pair for the return value of a wrapped one-shot C++ coroutine.
//
// This is an implementation detail and is not exposed to the programmer. It must match the
// definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncFutureChannel<Fut, Out> {
    // The receiving end.
    future: Fut,
    // The sending end.
    sender: CxxAsyncSender<Out>,
}

// A sender/receiver pair for the return value of a wrapped C++ multi-shot coroutine.
//
// This is an implementation detail and is not exposed to the programmer. It must match the
// definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncStreamChannel<Stm, Item>
where
    Stm: Stream<Item = CxxAsyncResult<Item>>,
{
    // The receiving end.
    future: Stm,
    // The sending end.
    sender: CxxAsyncSender<Item>,
}

// Allows the Rust polling interface to drive C++ tasks to completion.
//
// This is needed by the Folly backend, to allow awaiting semifutures.
#[derive(Clone)]
#[doc(hidden)]
pub struct Execlet(Arc<RustExeclet>);

struct WeakExeclet(Weak<RustExeclet>);

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

    fn downgrade(&self) -> WeakExeclet {
        WeakExeclet(Arc::downgrade(&self.0))
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
        let mut guard = self.0 .0.lock().unwrap();
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
            guard = self.0 .0.lock().unwrap();
        }

        // Unlock.
        guard.running = false;
    }

    // Submits a task to this execlet.
    fn submit(&self, task: ExecletTask) {
        let mut this = self.0 .0.lock().unwrap();
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

// Data for each execlet.
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

struct ExecletReaper {
    execlets: Mutex<ExecletReaperQueue>,
    cond: Condvar,
}

struct ExecletReaperQueue {
    new: Vec<WeakExeclet>,
    old: Vec<Execlet>,
}

impl ExecletReaper {
    fn get() -> Arc<ExecletReaper> {
        static INSTANCE: OnceCell<Arc<ExecletReaper>> = OnceCell::new();
        (*INSTANCE.get_or_init(|| {
            let reaper = Arc::new(ExecletReaper {
                execlets: Mutex::new(ExecletReaperQueue {
                    new: vec![],
                    old: vec![],
                }),
                cond: Condvar::new(),
            });
            let reaper_ = reaper.clone();
            thread::spawn(move || ExecletReaper::run(reaper_));
            reaper
        }))
        .clone()
    }

    fn add(&self, execlet: Execlet) {
        let mut execlets = self.execlets.lock().unwrap();
        execlets.new.push(execlet.downgrade());

        // Go ahead and start running the execlet if the reaper is sleeping. This makes sure that
        // we properly handle the following sequence of events:
        //
        // 1. User code drops the `CxxAsyncReceiver` from a task T and `CxxAsyncReceiver::drop()`
        //    starts running, but doesn't get here yet.
        // 2. The wrapped C++ future resolves on some C++ executor on another thread and submits a
        //    task U to the execlet.
        // 3. The original Waker is invoked and the Rust executor makes a note to resume task T.
        //    Because T is already running and has dropped the receiver, this does *not* cause the
        //    execlet to run.
        // 4. We add the execlet to this reaper.
        //
        // In this case, we have to make sure that we run task U, even though an execlet reaper
        // waker was never invoked.
        self.cond.notify_all();
    }

    fn run(self: Arc<Self>) {
        loop {
            let mut execlets = self.execlets.lock().unwrap();
            while execlets.is_empty() {
                execlets = self.cond.wait(execlets).unwrap();
            }

            let execlets_to_process = execlets.take();
            for execlet in execlets_to_process {
                drop(execlets);
                unsafe {
                    execlet.run(&mut Context::from_waker(&Waker::from_raw(
                        ExecletReaperWaker {
                            execlet: execlet.clone(),
                        }
                        .into_raw(),
                    )));
                }
                execlets = self.execlets.lock().unwrap();
                execlets.old.push(execlet);
            }
        }
    }

    fn wake(&self) {
        self.cond.notify_all();
    }
}

impl ExecletReaperQueue {
    fn is_empty(&self) -> bool {
        self.new.is_empty() && self.old.is_empty()
    }

    fn take(&mut self) -> Vec<Execlet> {
        let mut execlets = mem::take(&mut self.old);
        for execlet in mem::take(&mut self.new).into_iter() {
            if let Some(execlet) = execlet.0.upgrade() {
                execlets.push(Execlet(execlet));
            }
        }
        execlets
    }
}

#[derive(Clone)]
struct ExecletReaperWaker {
    execlet: Execlet,
}

impl ExecletReaperWaker {
    unsafe fn from_raw(data: *const ()) -> ExecletReaperWaker {
        ExecletReaperWaker {
            execlet: Execlet(Arc::from_raw(data as *const RustExeclet)),
        }
    }

    fn into_raw(self) -> RawWaker {
        RawWaker::new(
            Arc::into_raw(self.execlet.0) as *const (),
            &EXECLET_REAPER_WAKER_VTABLE,
        )
    }

    fn wake(self) {
        ExecletReaper::get().wake();
    }
}

unsafe fn execlet_reaper_waker_clone(data: *const ()) -> RawWaker {
    let execlet_reaper_waker = ExecletReaperWaker::from_raw(data);
    let waker = execlet_reaper_waker.clone().into_raw();
    mem::forget(execlet_reaper_waker);
    waker
}

unsafe fn execlet_reaper_waker_wake(data: *const ()) {
    ExecletReaperWaker::from_raw(data).wake()
}

unsafe fn execlet_reaper_waker_wake_by_ref(data: *const ()) {
    let waker = ExecletReaperWaker::from_raw(data);
    waker.clone().wake();
    mem::forget(waker);
}

unsafe fn execlet_reaper_waker_drop(data: *const ()) {
    drop(ExecletReaperWaker::from_raw(data))
}

static EXECLET_REAPER_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    execlet_reaper_waker_clone,
    execlet_reaper_waker_wake,
    execlet_reaper_waker_wake_by_ref,
    execlet_reaper_waker_drop,
);

// The single-producer/single-consumer channel type that future and stream implementations use to
// pass values between the two languages.
//
// We can't use a `futures::channel::mpsc` channel because it can deadlock. With a standard MPSC
// channel, if we try to send a value when the buffer is full and the receiving end is woken up and
// then tries to receive the value, a deadlock occurs, as the MPSC channel doesn't drop locks before
// calling the waker.
struct SpscChannel<T>(Arc<Mutex<SpscChannelImpl<T>>>);

// Data for each SPSC channel.
struct SpscChannelImpl<T> {
    // The waker waiting on the value.
    //
    // This can either be the sending end waiting for the receiving end to receive a previously-sent
    // value (for streams only) or the receiving end waiting for the sending end to post a value.
    waiter: Option<Waker>,
    // The value waiting to be read.
    value: Option<T>,
    // An exception from the C++ side that is to be delivered over to the Rust side.
    exception: Option<CxxAsyncException>,
    // True if the channel is closed; false otherwise.
    closed: bool,
}

impl<T> SpscChannel<T> {
    // Creates a new SPSC channel.
    fn new() -> SpscChannel<T> {
        SpscChannel(Arc::new(Mutex::new(SpscChannelImpl {
            waiter: None,
            value: None,
            exception: None,
            closed: false,
        })))
    }

    // Marks the channel as closed. Only the sending end may call this.
    fn close(&self) {
        // Drop the lock before possibly calling the waiter because we could deadlock otherwise.
        let waiter;
        {
            let mut this = self.0.lock().unwrap();
            if this.closed {
                panic!("Attempted to close an `SpscChannel` that's already closed!")
            }
            this.closed = true;
            waiter = this.waiter.take();
        }

        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }

    // Attempts to send a value. If this channel already has a value yet to be read, this function
    // returns false. Otherwise, it calls the provided closure to retrieve the value and returns
    // true.
    //
    // This callback-based design eliminates the requirement to return the original value if the
    // send fails.
    fn try_send_value_with<F>(&self, context: Option<&Context>, getter: F) -> bool
    where
        F: FnOnce() -> T,
    {
        // Drop the lock before possibly calling the waiter because we could deadlock otherwise.
        let waiter;
        {
            let mut this = self.0.lock().unwrap();
            if this.value.is_none() {
                this.value = Some(getter());
                waiter = this.waiter.take();
            } else if context.is_some() && this.waiter.is_some() {
                panic!("Only one task may block on a `SpscChannel`!")
            } else {
                if let Some(context) = context {
                    this.waiter = Some((*context.waker()).clone());
                }
                return false;
            }
        }

        if let Some(waiter) = waiter {
            waiter.wake();
        }
        true
    }

    // Raises an exception. This is synchronous and thus should never fail. It must only be called
    // once, or not at all, for a given `SpscSender`.
    fn send_exception(&self, exception: CxxAsyncException) {
        // Drop the lock before possibly calling the waiter because we could deadlock otherwise.
        let waiter = {
            let mut this = self.0.lock().unwrap();
            debug_assert!(this.exception.is_none());
            this.exception = Some(exception);
            this.waiter.take()
        };

        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }

    // Attempts to receive a value. Returns `Poll::Pending` if no value is available and the channel
    // isn't closed. Returns `Poll::Ready(None)` if the channel is closed. Otherwise, receives a
    // value and returns `Poll::Ready(Some)`.
    fn recv(&self, cx: &Context) -> Poll<Option<CxxAsyncResult<T>>> {
        // Drop the lock before possibly calling the waiter because we could deadlock otherwise.
        let (result, waiter);
        {
            let mut this = self.0.lock().unwrap();
            match this.value.take() {
                Some(value) => {
                    result = Ok(value);
                    waiter = this.waiter.take();
                }
                None => match this.exception.take() {
                    Some(exception) => {
                        result = Err(exception);
                        waiter = this.waiter.take();
                    }
                    None if this.closed => return Poll::Ready(None),
                    None => {
                        this.waiter = Some((*cx.waker()).clone());
                        return Poll::Pending;
                    }
                },
            }
        }

        if let Some(waiter) = waiter {
            waiter.wake();
        }
        Poll::Ready(Some(result))
    }
}

impl<T> Clone for SpscChannel<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// The concrete type of the stream that wraps a C++ coroutine, either one-shot (future) or
// multi-shot (stream).
//
// The programmer only interacts with this abstractly behind a `Box<dyn Future>` or
// `Box<dyn Stream>` trait object, so this type is considered an implementation detail. It must be
// public because the `bridge_stream` macro needs to name it.
#[doc(hidden)]
pub struct CxxAsyncReceiver<Item> {
    // The SPSC channel to receive on.
    receiver: SpscChannel<Item>,
    // Any execlet that must be driven when receiving.
    execlet: Option<Execlet>,
}

// The concrete type of the sending end of a stream.
//
// This must be public because the `bridge_stream` macro needs to name it.
#[doc(hidden)]
#[repr(transparent)]
pub struct CxxAsyncSender<Item>(*mut SpscChannel<Item>);

impl<Item> Drop for CxxAsyncSender<Item> {
    fn drop(&mut self) {
        unsafe { drop(Box::from_raw(self.0)) }
    }
}

// This is a little weird in that `CxxAsyncReceiver` behaves as a oneshot if it's treated as a
// Future and an SPSC stream if it's treated as a Stream. But since the programmer only ever
// interacts with these objects behind boxed trait objects that only expose one of the two traits,
// it's not a problem.
impl<Item> Stream for CxxAsyncReceiver<Item> {
    type Item = CxxAsyncResult<Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(ref execlet) = self.execlet {
            execlet.run(cx);
        }
        self.receiver.recv(cx)
    }
}

impl<Output> Future for CxxAsyncReceiver<Output> {
    type Output = CxxAsyncResult<Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(ref execlet) = self.execlet {
            execlet.run(cx);
        }
        match self.receiver.recv(cx) {
            Poll::Ready(Some(Ok(value))) => Poll::Ready(Ok(value)),
            Poll::Ready(Some(Err(exception))) => Poll::Ready(Err(exception)),
            Poll::Ready(None) => {
                // This should never happen, because a future should never be polled again after
                // returning `Ready`.
                panic!("Attempted to use a stream as a future!")
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<Item> From<SpscChannel<Item>> for CxxAsyncReceiver<Item> {
    fn from(receiver: SpscChannel<Item>) -> Self {
        CxxAsyncReceiver {
            receiver,
            execlet: None,
        }
    }
}

impl<Item> Drop for CxxAsyncReceiver<Item> {
    fn drop(&mut self) {
        let execlet = match self.execlet {
            Some(ref execlet) => execlet,
            None => return,
        };
        let receiver = self.receiver.0.lock().unwrap();
        if receiver.closed {
            return;
        }
        ExecletReaper::get().add((*execlet).clone());
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
/// You should not need to implement this manually; it's automatically implemented by the `bridge`
/// macro.
pub trait IntoCxxAsyncFuture: Sized {
    /// The type of the value yielded by the future.
    type Output;

    /// Wraps a Rust Future that directly returns the output type.
    ///
    /// Use this when you aren't interested in propagating errors to C++ as exceptions.
    fn infallible<Fut>(future: Fut) -> Self
    where
        Fut: Future<Output = Self::Output> + Send + 'static,
    {
        Self::fallible(async move { Ok(future.await) })
    }

    /// Wraps a Rust Future that returns the output type, wrapped in a `CxxAsyncResult`.
    ///
    /// Use this when you have error values that you want to turn into exceptions on the C++ side.
    fn fallible<Fut>(future: Fut) -> Self
    where
        Fut: Future<Output = CxxAsyncResult<Self::Output>> + Send + 'static;
}

/// Wraps an arbitrary Rust Stream in a boxed `cxx-async` stream so that it can be returned to C++.
///
/// You should not need to implement this manually; it's automatically implemented by the
/// `bridge_stream` macro.
pub trait IntoCxxAsyncStream: Sized {
    /// The type of the values yielded by the stream.
    type Item;

    /// Wraps a Rust Stream that directly yields items of the output type.
    ///
    /// Use this when you aren't interested in propagating errors to C++ as exceptions.
    fn infallible<Stm>(stream: Stm) -> Self
    where
        Stm: Stream<Item = Self::Item> + Send + 'static,
        Stm::Item: 'static,
    {
        Self::fallible(stream.map(Ok))
    }

    /// Wraps a Rust Stream that yields items of the output type, wrapped in `CxxAsyncResult`s.
    ///
    /// Use this when you have error values that you want to turn into exceptions on the C++ side.
    fn fallible<Stm>(stream: Stm) -> Self
    where
        Stm: Stream<Item = CxxAsyncResult<Self::Item>> + Send + 'static;
}

// Creates a new oneshot sender/receiver pair for a future.
//
// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
#[doc(hidden)]
pub unsafe extern "C" fn future_channel<Fut, Out>(
    out_oneshot: *mut CxxAsyncFutureChannel<Fut, Out>,
    execlet: *mut RustExeclet,
) where
    Fut: From<CxxAsyncReceiver<Out>> + Future<Output = CxxAsyncResult<Out>>,
{
    let channel = SpscChannel::new();
    let oneshot = CxxAsyncFutureChannel {
        sender: CxxAsyncSender(Box::into_raw(Box::new(channel.clone()))),
        future: CxxAsyncReceiver::<Out> {
            receiver: channel,
            execlet: Some(Execlet::from_raw_ref(execlet)),
        }.into(),
    };
    ptr::copy_nonoverlapping(&oneshot, out_oneshot, 1);
    mem::forget(oneshot);
}

// Creates a new multi-shot sender/receiver pair for a stream.
//
// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
#[doc(hidden)]
pub unsafe extern "C" fn stream_channel<Stm, Item>(
    out_stream: *mut CxxAsyncStreamChannel<Stm, Item>,
    execlet: *mut RustExeclet,
) where
    Stm: From<CxxAsyncReceiver<Item>> + Stream<Item = CxxAsyncResult<Item>>,
{
    let channel = SpscChannel::new();
    let stream = CxxAsyncStreamChannel {
        sender: CxxAsyncSender(Box::into_raw(Box::new(channel.clone()))),
        future: CxxAsyncReceiver {
            receiver: channel,
            execlet: Some(Execlet::from_raw_ref(execlet)),
        }
        .into(),
    };
    ptr::copy_nonoverlapping(&stream, out_stream, 1);
    mem::forget(stream);
}

// C++ calls this to yield a value for a one-shot coroutine (future).
//
// SAFETY: This is a low-level function called by our C++ code.
//
// Takes ownership of the value. The caller must not call its destructor.
//
// This function always closes the channel and returns `SEND_RESULT_FINISHED`.
//
// If `status` is `FUTURE_STATUS_COMPLETE`, then the given value is sent; otherwise, if `status` is
// `FUTURE_STATUS_ERROR`, `value` must point to an exception string. `FUTURE_STATUS_RUNNING` is
// illegal, because that value is only for streams, not futures.
//
// The `waker_data` parameter should always be null, because a one-shot coroutine should never
// block on yielding a value.
//
// Any errors when sending are dropped on the floor. This is the right behavior because futures
// can be legally dropped in Rust to signal cancellation.
#[doc(hidden)]
pub unsafe extern "C" fn sender_future_send<Item>(
    this: &mut CxxAsyncSender<Item>,
    status: u32,
    value: *const u8,
    waker_data: *const u8,
) -> u32 {
    debug_assert!(waker_data.is_null());

    let this = this.0.as_mut().expect("Where's the SPSC sender?");
    match status {
        FUTURE_STATUS_COMPLETE => {
            // This is a one-shot sender, so sending must always succeed.
            let sent = this.try_send_value_with(None, || unpack_value::<Item>(value));
            debug_assert!(sent);
        }
        FUTURE_STATUS_ERROR => this.send_exception(unpack_exception(value)),
        _ => unreachable!(),
    }

    this.close();
    SEND_RESULT_FINISHED
}

// C++ calls this to yield a value for a multi-shot coroutine (stream).
//
// SAFETY: This is a low-level function called by our C++ code.
//
// Takes ownership of the value if and only if it was successfully sent (otherwise, leaves it
// alone). The caller must not call its destructor on a successful send.
//
// This function returns `SEND_RESULT_SENT` if the value was successfully sent, `SEND_RESULT_WAIT`
// if the value wasn't sent because another value was already in the slot (in which case the task
// will need to go to sleep), and `SEND_RESULT_FINISHED` if the channel was closed.
//
// If `status` is `FUTURE_STATUS_COMPLETE`, then the channel is closed, the value is ignored, and
// this function immediately returns `SEND_RESULT_FINISHED`. Note that this behavior is different
// from `sender_future_send`, which actually sends the value if `status` is
// `FUTURE_STATUS_COMPLETE`.
//
// If `waker_data` is present, this identifies the coroutine handle that will be awakened if the
// channel is currently full.
//
// Any errors when sending are dropped on the floor. This is because futures can be legally dropped
// in Rust to signal cancellation.
#[doc(hidden)]
pub unsafe extern "C" fn sender_stream_send<Item>(
    this: &mut CxxAsyncSender<Item>,
    status: u32,
    value: *const u8,
    waker_data: *const u8,
) -> u32 {
    let (waker, context);
    if waker_data.is_null() {
        context = None;
    } else {
        waker = Waker::from_raw(RawWaker::new(
            waker_data as *const (),
            &CXXASYNC_WAKER_VTABLE,
        ));
        context = Some(Context::from_waker(&waker));
    }

    let this = this.0.as_mut().expect("Where's the SPSC sender?");
    match status {
        FUTURE_STATUS_COMPLETE => {
            this.close();
            SEND_RESULT_FINISHED
        }
        FUTURE_STATUS_RUNNING => {
            let sent = this.try_send_value_with(context.as_ref(), || unpack_value::<Item>(value));
            if sent {
                SEND_RESULT_SENT
            } else {
                SEND_RESULT_WAIT
            }
        }
        FUTURE_STATUS_ERROR => {
            this.send_exception(unpack_exception(value));
            this.close();
            SEND_RESULT_FINISHED
        }
        _ => unreachable!(),
    }
}

// C++ calls this to destroy a sender.
//
// SAFETY: This is a low-level function called by our C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn sender_drop<Item>(_: CxxAsyncSender<Item>) {
    // Destructor automatically runs.
}

unsafe fn unpack_value<Output>(value: *const u8) -> Output {
    let mut staging: MaybeUninit<Output> = MaybeUninit::uninit();
    ptr::copy_nonoverlapping(value as *const Output, staging.as_mut_ptr(), 1);
    staging.assume_init()
}

unsafe fn unpack_exception(value: *const u8) -> CxxAsyncException {
    let string = CStr::from_ptr(value as *const i8);
    CxxAsyncException::new(string.to_string_lossy().into_owned().into_boxed_str())
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

// C++ calls this to drop a Rust future.
//
// SAFETY:
// * This is a low-level function called by our C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn future_drop<Fut>(future: *mut Fut) {
    ptr::drop_in_place(future);
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

// Reexports for the `#[bridge]` macro to use internally. Users of this crate shouldn't use these;
// they should import the `futures` crate directly.
#[doc(hidden)]
pub mod private {
    pub use futures::future::BoxFuture;
    pub use futures::stream::{BoxStream, Stream};
}
