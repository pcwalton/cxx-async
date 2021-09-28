// cxx-async2/src/main.rs
//
//! `cxx-async` is a Rust crate that extends the cxx library to provide seamless interoperability
//! between asynchronous Rust code using `async`/`await` and C++20 coroutines using `co_await`. If
//! your C++ code is asynchronous, `cxx-async` can provide a more convenient, and potentially more
//! efficient, alternative to callbacks. You can freely convert between C++ coroutines and Rust
//! futures and await one from the other.
//! 
//! It's important to emphasize what `cxx-async` isn't: it isn't a C++ binding to Tokio or any
//! other Rust I/O library. Nor is it a Rust binding to `boost::asio` or similar. Such bindings
//! could in principle be layered on top of `cxx-async` if desired, but this crate doesn't provide
//! them out of the box. (Note that this is a tricky problem even in principle, since Rust async
//! I/O code is generally tightly coupled to a single library such as Tokio, in much the same way
//! C++ async I/O code tends to be tightly coupled to libraries like `boost::asio`.) If you're
//! writing server code, you can still use `cxx-async`, but you will need to ensure that both the
//! Rust and C++ sides run separate I/O executors.
//! 
//! `cxx-async` aims for compatibility with popular C++ coroutine support libraries. Right now,
//! both the lightweight [`cppcoro`](https://github.com/lewissbaker/cppcoro) and the more
//! comprehensive [Folly](https://github.com/facebook/folly/) are supported. Patches are welcome to
//! support others.

#![warn(missing_docs)]

extern crate link_cplusplus;

use crate::ffi::{
    suspended_coroutine_clone, suspended_coroutine_drop, suspended_coroutine_wake,
    suspended_coroutine_wake_by_ref,
};
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

#[doc(hidden)]
pub use paste::paste;
#[doc(hidden)]
pub use pin_utils::unsafe_pinned;

// Bridged glue functions.
#[cxx::bridge]
mod ffi {
    // General

    #[namespace = "cxx::async"]
    unsafe extern "C++" {
        include!("cxx_async_waker.h");
        unsafe fn suspended_coroutine_clone(waker_data: *mut u8) -> *mut u8;
        unsafe fn suspended_coroutine_wake(waker_data: *mut u8);
        unsafe fn suspended_coroutine_wake_by_ref(waker_data: *mut u8);
        unsafe fn suspended_coroutine_drop(waker_data: *mut u8);
    }
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
    pub execlet: *mut u8,
    pub execlet_submit: *mut u8,
    pub execlet_send: *mut u8,
}

unsafe impl Send for CxxAsyncVtable {}
unsafe impl Sync for CxxAsyncVtable {}

// A sender/receiver pair for the return value of a wrapped C++ coroutine.
// 
// This is an implementation detail and is not exposed to the programmer. It must match the
// definition in `cxx_async.h`.
#[repr(C)]
#[doc(hidden)]
pub struct CxxAsyncOneshot<Future, Sender> {
    // The receiving end.
    future: Box<Future>,
    // The sending end.
    sender: Box<Sender>,
}

// The concrete type of the future that wraps a C++ coroutine.
//
// The programmer only interacts with this abstractly behind a `Box<dyn Future>` trait object, so
// this type is considered an implementation detail. It must be public because the
// `define_cxx_future!` macro needs to name it.
#[doc(hidden)]
pub struct CxxAsyncReceiver<Output>(OneshotReceiver<CxxAsyncResult<Output>>);

impl<Output> Future for CxxAsyncReceiver<Output> {
    type Output = CxxAsyncResult<Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Future::poll(Pin::new(&mut self.0), cx) {
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
        Self(receiver)
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

// A trait very similar to `Future`, but with two differences:
// 1. It doesn't require `self` to be pinned, because C++ has no notion of this.
// 2. The return value is wrapped in `CxxAsyncResult`.
//
// This is an implementation detail used when C++ waits for a Rust future.
#[doc(hidden)]
pub trait CxxAsyncFuture {
    type Output;
    fn poll(&mut self, context: &mut Context) -> Poll<CxxAsyncResult<Self::Output>>;
}

// An execlet and a future that extracts the return value from it.
//
// Execlets are used to drive Folly semi-futures from the Rust polling interface.
// 
// This is an implementation detail and is not exposed to the programmer. It must match the
// definition in `cxx_async.h`.
#[doc(hidden)]
#[repr(C)]
pub struct CxxAsyncExecletBundle<Future, Execlet> {
    // The receiving end.
    future: Box<Future>,
    // The driver.
    execlet: Box<Execlet>,
}

// The mechanism that provides a Rust `Future` interface to a Folly semi-future.
//
// This is a simple Folly executor that drives a Rust future to completion. It's an implementation
// detail and not directly exposed to the programmer.
#[doc(hidden)]
#[derive(Clone)]
pub struct Execlet<Output>(Arc<Mutex<ExecletData<Output>>>)
where
    Output: Clone;

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

// Internal state for an execlet.
struct ExecletData<Output>
where
    Output: Clone,
{
    // Tasks waiting to run.
    runqueue: VecDeque<ExecletTask>,
    // When done, the output value of the C++ task is stored here.
    result: Option<CxxAsyncResult<Output>>,
    // A Rust Waker that will be informed when we need to wake up and run some tasks in our
    // runqueue or yield the result.
    waker: Option<Waker>,
    // True if we're running; false otherwise. This flag is necessary to avoid deadlocks resulting
    // from recursive invocations.
    running: bool,
}

// The concrete type of the Rust Future wrapping an execlet.
//
// This is an implementation detail and is not exposed to the programmer.
#[doc(hidden)]
pub struct ExecletFuture<Output>
where
    Output: Clone,
{
    // The wrapped execlet that we take the result value from.
    execlet: Execlet<Output>,
}

impl<Output> Execlet<Output>
where
    Output: Clone,
{
    fn new() -> Execlet<Output> {
        Execlet(Arc::new(Mutex::new(ExecletData {
            runqueue: VecDeque::new(),
            result: None,
            waker: None,
            running: false,
        })))
    }

    fn bundle() -> (ExecletFuture<Output>, Self) {
        let execlet = Self::new();
        let future = ExecletFuture {
            execlet: execlet.clone(),
        };
        (future, execlet)
    }
}

impl<Output> ExecletFuture<Output>
where
    Output: Clone,
{
    // Creates a new ExecletFuture that will take its result value from the given Execlet.
    #[doc(hidden)]
    pub fn new(execlet: Execlet<Output>) -> Self {
        ExecletFuture { execlet }
    }
}

impl<Output> Future for ExecletFuture<Output>
where
    Output: Clone,
{
    type Output = CxxAsyncResult<Output>;

    // Wake up and run as many tasks as we can.
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        // Lock.
        let mut guard = self.execlet.0.lock().unwrap();
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
            guard = self.execlet.0.lock().unwrap();
        }

        // Fill in the result if necessary.
        guard.running = false;
        match guard.result.take() {
            Some(result) => Poll::Ready(result),
            None => Poll::Pending,
        }
    }
}

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
        return Self::fallible(async move { Ok(future.await) });
    }

    /// Wraps a Rust Future that returns the output type, wrapped in a `CxxAsyncResult`.
    ///
    /// Use this when you have error values that you want to turn into exceptions on the C++ side.
    fn fallible<Fut>(future: Fut) -> Box<Self>
    where
        Fut: Future<Output = CxxAsyncResult<Self::Output>> + Send + 'static;
}

/// Defines a shared future type that can be awaited by asynchronous code in the other language.
///
/// The first argument is the name of the future, minus the `RustFuture` prefix. The second argument
/// is the name of the Rust output type, which will be automatically converted to the corresponding
/// C++ type. For example, to define a shared future named `RustFutureF64` that returns an `f64`,
/// you could write `define_cxx_future!(F64, f64);`.
#[macro_export]
macro_rules! define_cxx_future {
    ($name:ident, $output:ty) => {
        ::cxx_async2::paste! {
            /// A future shared between Rust and C++.
            pub struct [<RustFuture $name>] {
                // FIXME(pcwalton): Unfortunately, as far as I can tell this has to be double-boxed
                // because we need the `RustFuture` type to be Sized.
                future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>,
            }

            // The wrapper for the sending end.
            #[repr(transparent)]
            #[doc(hidden)]
            pub struct [<RustSender $name>](Option<::futures::channel::oneshot::Sender<
                ::cxx_async2::CxxAsyncResult<$output>>>);

            // A type alias for the receiving end (i.e. the concrete future type).
            type [<RustReceiver $name>] = ::cxx_async2::CxxAsyncReceiver<$output>;

            /*
            #[repr(C)]
            #[doc(hidden)]
            pub struct [<RustOneshot $name>] {
                future: Box<[<RustFuture $name>]>,
                sender: Box<[<RustSender $name>]>,
            }

            #[repr(transparent)]
            pub struct [<RustExecletFuture $name>](::cxx_async2::ExecletFuture<$output>);
            */

            // The wrapped execlet for this future.
            #[repr(transparent)]
            pub struct [<RustExeclet $name>](::cxx_async2::Execlet<$output>);

            impl [<RustFuture $name>] {
                // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
                // 1. The struct does not implement Drop (other than for debugging, which doesn't
                // move the field).
                // 2. The struct doesn't implement Unpin.
                // 3. The struct isn't `repr(packed)`.
                ::cxx_async2::unsafe_pinned!(future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>);
            }

            // Define how to box up a future.
            impl ::cxx_async2::IntoCxxAsyncFuture for [<RustFuture $name>] {
                type Output = $output;
                fn fallible<Fut>(future: Fut) -> Box<Self> where Fut:
                        ::std::future::Future<Output = ::cxx_async2::CxxAsyncResult<$output>> +
                            Send + 'static {
                    Box::new([<RustFuture $name>] {
                        future: Box::pin(future),
                    })
                }
            }

            // Implement the Rust Future trait.
            impl ::std::future::Future for [<RustFuture $name>] {
                type Output = ::cxx_async2::CxxAsyncResult<$output>;
                fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                        -> ::std::task::Poll<Self::Output> {
                    self.future().poll(cx)
                }
            }

            // Implement the CxxAsyncFuture trait that doesn't require pinning.
            impl ::cxx_async2::CxxAsyncFuture for [<RustFuture $name>] {
                type Output = $output;
                fn poll(&mut self, context: &mut ::std::task::Context)
                        -> ::std::task::Poll<::cxx_async2::CxxAsyncResult<Self::Output>> {
                    ::std::future::Future::poll(::std::pin::Pin::new(&mut self.future), context)
                }
            }

            // Implement sending for the Sender trait.
            //
            // FIXME(pcwalton): Not sure if we need this?
            impl ::cxx_async2::RustSender for [<RustSender $name>] {
                type Output = $output;
                fn send(&mut self, value: ::cxx_async2::CxxAsyncResult<$output>) {
                    self.0.take().unwrap().send(value).unwrap()
                }
            }

            // Define how to wrap concrete receivers in the `RustFuture` type.
            impl ::std::convert::From<[<RustReceiver $name>]> for [<RustFuture $name>] {
                fn from(receiver: [<RustReceiver $name>]) -> Self {
                    Self {
                        future: Box::pin(receiver),
                    }
                }
            }

            // Define how to wrap raw oneshot senders in the `RustSender` type.
            impl ::std::convert::From<::futures::channel::oneshot::Sender<
                    ::cxx_async2::CxxAsyncResult<$output>>> for [<RustSender $name>] {
                fn from(sender: ::futures::channel::oneshot::Sender<::cxx_async2::CxxAsyncResult<
                        $output>>) -> Self {
                    Self(Some(sender))
                }
            }

            // Define how to wrap raw Execlets in the `RustExeclet` type we just defined.
            impl ::std::convert::From<::cxx_async2::Execlet<$output>> for [<RustExeclet $name>] {
                fn from(execlet: ::cxx_async2::Execlet<$output>) -> Self {
                    Self(execlet)
                }
            }

            // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
            impl [<RustFuture $name>] {
                pub fn infallible<Fut>(future: Fut) -> Box<Self>
                        where Fut: ::std::future::Future<Output = $output> + Send + 'static {
                    <[<RustFuture $name>] as ::cxx_async2::IntoCxxAsyncFuture>::infallible(future)
                }

                pub fn fallible<Fut>(future: Fut) -> Box<Self>
                        where Fut: ::std::future::Future<Output =
                            ::cxx_async2::CxxAsyncResult<$output>> + Send + 'static {
                    <[<RustFuture $name>] as ::cxx_async2::IntoCxxAsyncFuture>::fallible(future)
                }
            }

            // The C++ bridge calls this to destroy a `RustSender`.
            //
            // I'm not sure if this can ever legitimately happen, but C++ wants to link to this
            // function anyway, so let's provide it.
            //
            // SAFETY: This is a raw FFI function called by `cxx`. `cxx` ensures that `ptr` is a
            // valid Box.
            #[no_mangle]
            pub unsafe extern "C" fn [<cxxasync_drop_box_rust_sender_ $name>](ptr: *mut
                    Box<[<RustSender $name>]>) {
                let mut boxed: ::std::mem::MaybeUninit<Box<[<RustSender $name>]>> =
                    ::std::mem::MaybeUninit::uninit();
                ::std::ptr::copy_nonoverlapping(ptr, boxed.as_mut_ptr(), 1);
                drop(boxed.assume_init());
            }

            // The C++ bridge calls this to destroy an `Execlet`.
            //
            // SAFETY: This is a raw FFI function called by `cxx`. `cxx` ensures that `ptr` is a
            // valid Box.
            #[no_mangle]
            #[doc(hidden)]
            pub unsafe extern "C" fn [<cxxasync_drop_box_rust_execlet_ $name>](ptr: *mut
                    Box<[<RustExeclet $name>]>) {
                let mut boxed: ::std::mem::MaybeUninit<Box<[<RustExeclet $name>]>> =
                    ::std::mem::MaybeUninit::uninit();
                ::std::ptr::copy_nonoverlapping(ptr, boxed.as_mut_ptr(), 1);
                drop(boxed.assume_init());
            }

            #[doc(hidden)]
            #[no_mangle]
            pub static [<cxx_async_vtable_ $name>]: ::cxx_async2::CxxAsyncVtable =
                ::cxx_async2::CxxAsyncVtable {
                    channel: ::cxx_async2::channel::<[<RustFuture $name>],
                        [<RustSender $name>], [<RustReceiver $name>], $output> as *mut u8,
                    sender_send: ::cxx_async2::sender_send::<[<RustSender $name>],
                        $output> as *mut u8,
                    future_poll: ::cxx_async2::future_poll::<$output,
                        [<RustFuture $name>]> as *mut u8,
                    execlet: ::cxx_async2::execlet_bundle::<[<RustFuture $name>],
                        [<RustExeclet $name>], $output> as *mut u8,
                    execlet_submit: ::cxx_async2::execlet_submit::<$output> as *mut u8,
                    execlet_send: ::cxx_async2::execlet_send::<$output> as *mut u8,
                };
        }
    };
}

// Creates a new oneshot sender/receiver pair.
//
// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
#[doc(hidden)]
pub unsafe extern "C" fn channel<Future, Sender, Receiver, Output>(
    out_oneshot: *mut CxxAsyncOneshot<Future, Sender>,
) where
    Future: From<Receiver>,
    Receiver: From<OneshotReceiver<CxxAsyncResult<Output>>>,
    Sender: From<OneshotSender<CxxAsyncResult<Output>>>,
{
    let (sender, receiver) = oneshot::channel();
    let oneshot = CxxAsyncOneshot {
        sender: Box::new(sender.into()),
        future: Box::new(Receiver::from(receiver).into()),
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
#[doc(hidden)]
pub unsafe extern "C" fn sender_send<Sender, Output>(
    this: &mut Sender,
    status: u32,
    value: *const u8,
) where
    Sender: RustSender<Output = Output>,
{
    this.send(unpack_value_to_send(status, value))
}

// C++ calls this to poll a wrapped Rust future.
//
// SAFETY: This is a low-level function called by our C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn future_poll<Output, Future>(
    this: &mut Future,
    result: *mut u8,
    waker_data: *const u8,
) -> u32
where
    Future: CxxAsyncFuture<Output = Output>,
{
    let waker = Waker::from_raw(RawWaker::new(
        waker_data as *const (),
        &CXXASYNC_WAKER_VTABLE,
    ));
    let mut context = Context::from_waker(&waker);
    match this.poll(&mut context) {
        Poll::Ready(Ok(value)) => {
            ptr::copy_nonoverlapping(&value, result as *mut Output, 1);
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

// Generates a new future/execlet pair.
//
// SAFETY: This is a low-level function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
#[doc(hidden)]
pub unsafe extern "C" fn execlet_bundle<Future, Exec, Output>(
    out_bundle: *mut CxxAsyncExecletBundle<Future, Exec>,
) where
    Future: IntoCxxAsyncFuture<Output = Output>,
    Exec: From<Execlet<Output>>,
    Output: Clone + Send + 'static,
{
    let (future, execlet) = Execlet::<Output>::bundle();
    let bundle = CxxAsyncExecletBundle {
        future: Future::fallible(future),
        execlet: Box::new(execlet.into()),
    };
    ptr::copy_nonoverlapping(&bundle, out_bundle, 1);
    mem::forget(bundle);
}

// C++ calls this to submit a new task to the execlet.
//
// SAFETY: This is a low-level function called by our C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn execlet_submit<Output>(
    this: &Execlet<Output>,
    run: extern "C" fn(*mut u8),
    task_data: *mut u8,
) where
    Output: Clone,
{
    let mut this = this.0.lock().unwrap();
    this.runqueue.push_back(ExecletTask::new(run, task_data));
    if let Some(ref waker) = this.waker {
        // Avoid possible deadlocks.
        // FIXME(pcwalton): Is this necessary?
        let waker = (*waker).clone();
        drop(this);

        waker.wake_by_ref();
    }
}

// C++ calls this to place a final value in the appropriate slot in the execlet.
//
// SAFETY: This is a low-level function called by our C++ code.
#[doc(hidden)]
pub unsafe extern "C" fn execlet_send<Output>(
    this: &Execlet<Output>,
    status: u32,
    value: *const u8,
) where
    Output: Clone,
{
    let mut this = this.0.lock().unwrap();
    assert!(this.result.is_none());
    this.result = Some(unpack_value_to_send(status, value));

    // Don't do this if we're running, or we might end up in a situation where the waker tries
    // to poll us again, which is UB (and will deadlock in the C++ bindings).
    if !this.running {
        this.waker
            .as_ref()
            .expect("Send with no waker present?")
            .wake_by_ref();
    }
}

// Bumps the reference count on a suspended C++ coroutine.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_clone(address: *const ()) -> RawWaker {
    RawWaker::new(
        suspended_coroutine_clone(address as *mut () as *mut u8) as *mut () as *const (),
        &CXXASYNC_WAKER_VTABLE,
    )
}

// Resumes a suspended C++ coroutine and decrements its reference count.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake(address: *const ()) {
    suspended_coroutine_wake(address as *mut () as *mut u8)
}

// Resumes a suspended C++ coroutine without decrementing its reference count.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake_by_ref(address: *const ()) {
    suspended_coroutine_wake_by_ref(address as *mut () as *mut u8)
}

// Decrements the reference count on a suspended C++ coroutine.
//
// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_drop(address: *const ()) {
    suspended_coroutine_drop(address as *mut () as *mut u8)
}
