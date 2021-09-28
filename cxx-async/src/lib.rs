// cxx-async2/src/main.rs

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

pub const FUTURE_STATUS_PENDING: u32 = 0;
pub const FUTURE_STATUS_COMPLETE: u32 = 1;
pub const FUTURE_STATUS_ERROR: u32 = 2;

pub use paste::paste;
pub use pin_utils::unsafe_pinned;

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

pub static CXXASYNC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
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

// Must match the definition in `cxx_async.h`.
#[repr(C)]
pub struct CxxAsyncVtable {
    pub channel: *mut u8,
    pub sender_send: *mut u8,
    pub poll: *mut u8,
    pub execlet: *mut u8,
    pub submit: *mut u8,
    pub execlet_send: *mut u8,
}

unsafe impl Send for CxxAsyncVtable {}
unsafe impl Sync for CxxAsyncVtable {}

#[repr(C)]
pub struct CxxAsyncOneshot<Future, Sender> {
    pub future: Box<Future>,
    pub sender: Box<Sender>,
}

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

pub trait RustSender {
    type Output;
    fn send(&mut self, value: CxxAsyncResult<Self::Output>);
}

pub trait RustFuture {
    type Output;
    fn poll(&mut self, context: &mut Context) -> Poll<CxxAsyncResult<Self::Output>>;
}

#[repr(C)]
pub struct CxxAsyncExecletBundle<Future, Execlet> {
    pub future: Box<Future>,
    pub execlet: Box<Execlet>,
}

#[derive(Clone)]
pub struct Execlet<Output>(Arc<Mutex<ExecletData<Output>>>)
where
    Output: Clone;

struct ExecletTask {
    run: unsafe extern "C" fn(*mut u8),
    data: *mut u8,
}

impl ExecletTask {
    fn new(run: unsafe extern "C" fn(*mut u8), data: *mut u8) -> Self {
        Self { run, data }
    }

    unsafe fn run(self) {
        (self.run)(self.data)
    }
}

unsafe impl Send for ExecletTask {}
unsafe impl Sync for ExecletTask {}

pub struct ExecletData<Output>
where
    Output: Clone,
{
    runqueue: VecDeque<ExecletTask>,
    result: Option<Output>,
    waker: Option<Waker>,
    running: bool,
}

// The receiving end of an execlet on the Rust side.
pub struct ExecletFuture<Output>
where
    Output: Clone,
{
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
    pub fn new(execlet: Execlet<Output>) -> Self {
        ExecletFuture { execlet }
    }
}

impl<Output> Future for ExecletFuture<Output>
where
    Output: Clone,
{
    // FIXME(pcwalton): Should be CxxAsyncResult
    type Output = Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut guard = self.execlet.0.lock().unwrap();
        debug_assert!(!guard.running);
        guard.running = true;

        guard.waker = Some((*cx.waker()).clone());
        while let Some(task) = guard.runqueue.pop_front() {
            drop(guard);
            unsafe {
                task.run();
            }
            guard = self.execlet.0.lock().unwrap();
        }

        guard.running = false;
        match guard.result.take() {
            Some(result) => Poll::Ready(result),
            None => Poll::Pending,
        }
    }
}

pub trait FutureWrap {
    type Output;

    fn from_infallible<Fut>(future: Fut) -> Box<Self>
    where
        Fut: Future<Output = Self::Output> + Send + 'static,
    {
        return Self::from_fallible(async move { Ok(future.await) });
    }

    fn from_fallible<Fut>(future: Fut) -> Box<Self>
    where
        Fut: Future<Output = CxxAsyncResult<Self::Output>> + Send + 'static;
}

#[macro_export]
macro_rules! define_cxx_future {
    ($name:ident, $output:ty) => {
        ::cxx_async2::paste! {
            pub struct [<RustFuture $name>] {
                future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>,
            }

            #[repr(transparent)]
            pub struct [<RustSender $name>](Option<::futures::channel::oneshot::Sender<
                ::cxx_async2::CxxAsyncResult<$output>>>);

            type [<RustReceiver $name>] = ::cxx_async2::CxxAsyncReceiver<$output>;

            #[repr(C)]
            pub struct [<RustOneshot $name>] {
                pub future: Box<[<RustFuture $name>]>,
                pub sender: Box<[<RustSender $name>]>,
            }

            #[repr(transparent)]
            pub struct [<RustExecletFuture $name>](::cxx_async2::ExecletFuture<$output>);

            #[repr(transparent)]
            pub struct [<RustExeclet $name>](::cxx_async2::Execlet<$output>);

            impl [<RustFuture $name>] {
                // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
                // (1) The struct does not implement Drop (other than for debugging, which doesn't
                // move the field).
                // (2) The struct doesn't implement Unpin.
                // (3) The struct isn't `repr(packed)`.
                ::cxx_async2::unsafe_pinned!(future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>);
            }

            impl ::cxx_async2::FutureWrap for [<RustFuture $name>] {
                type Output = $output;
                fn from_fallible<Fut>(future: Fut) -> Box<Self> where Fut:
                        ::std::future::Future<Output = ::cxx_async2::CxxAsyncResult<$output>> +
                            Send + 'static {
                    Box::new([<RustFuture $name>] {
                        future: Box::pin(future),
                    })
                }
            }

            impl ::cxx_async2::RustSender for [<RustSender $name>] {
                type Output = $output;
                fn send(&mut self, value: ::cxx_async2::CxxAsyncResult<$output>) {
                    self.0.take().unwrap().send(value).unwrap()
                }
            }

            impl ::cxx_async2::RustFuture for [<RustFuture $name>] {
                type Output = $output;
                fn poll(&mut self, context: &mut ::std::task::Context)
                        -> ::std::task::Poll<::cxx_async2::CxxAsyncResult<Self::Output>> {
                    ::std::future::Future::poll(::std::pin::Pin::new(&mut self.future), context)
                }
            }

            impl ::std::future::Future for [<RustFuture $name>] {
                type Output = ::cxx_async2::CxxAsyncResult<$output>;
                fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                        -> ::std::task::Poll<Self::Output> {
                    self.future().poll(cx)
                }
            }

            impl ::std::convert::From<[<RustReceiver $name>]> for [<RustFuture $name>] {
                fn from(receiver: [<RustReceiver $name>]) -> Self {
                    Self {
                        future: Box::pin(receiver),
                    }
                }
            }

            impl ::std::convert::From<::futures::channel::oneshot::Sender<
                    ::cxx_async2::CxxAsyncResult<$output>>> for [<RustSender $name>] {
                fn from(sender: ::futures::channel::oneshot::Sender<::cxx_async2::CxxAsyncResult<
                        $output>>) -> Self {
                    Self(Some(sender))
                }
            }

            /*
            impl ::std::convert::From<::cxx_async2::Execlet<$output>> for
                    [<RustExecletFuture $name>] {
                fn from(execlet: ::cxx_async2::Execlet<$output>) -> Self {
                    Self(::cxx_async2::ExecletFuture::new(execlet))
                }
            }
            */

            impl ::std::convert::From<::cxx_async2::Execlet<$output>> for [<RustExeclet $name>] {
                fn from(execlet: ::cxx_async2::Execlet<$output>) -> Self {
                    Self(execlet)
                }
            }

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

            // SAFETY: This is a raw FFI function called by `cxx`. `cxx` ensures that `ptr` is a
            // valid Box.
            #[no_mangle]
            pub unsafe extern "C" fn [<cxxasync_drop_box_rust_execlet_ $name>](ptr: *mut
                    Box<[<RustExeclet $name>]>) {
                let mut boxed: ::std::mem::MaybeUninit<Box<[<RustExeclet $name>]>> =
                    ::std::mem::MaybeUninit::uninit();
                ::std::ptr::copy_nonoverlapping(ptr, boxed.as_mut_ptr(), 1);
                drop(boxed.assume_init());
            }

            #[no_mangle]
            pub static [<cxx_async_vtable_ $name>]: ::cxx_async2::CxxAsyncVtable =
                ::cxx_async2::CxxAsyncVtable {
                    channel: ::cxx_async2::cxx_async_channel::<[<RustFuture $name>],
                        [<RustSender $name>], [<RustReceiver $name>], $output> as *mut u8,
                    sender_send: ::cxx_async2::cxx_async_sender_send::<[<RustSender $name>],
                        $output> as *mut u8,
                    poll: ::cxx_async2::cxx_async_future_poll::<$output, [<RustFuture $name>]> as
                        *mut u8,
                    execlet: ::cxx_async2::cxx_async_execlet_bundle::<[<RustFuture $name>],
                        [<RustExeclet $name>], $output> as *mut u8,
                    submit: ::cxx_async2::cxx_async_execlet_submit::<$output> as *mut u8,
                    execlet_send: ::cxx_async2::cxx_async_execlet_send::<$output> as *mut u8,
                };
        }
    };
}

// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
pub unsafe extern "C" fn cxx_async_channel<Future, Sender, Receiver, Output>(
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

// SAFETY: This is a raw FFI function called by our C++ code.
//
// Takes ownership of the value. The caller must not call its destructor.
pub unsafe extern "C" fn cxx_async_sender_send<Sender, Output>(
    this: &mut Sender,
    status: u32,
    value: *const u8,
) where
    Sender: RustSender<Output = Output>,
{
    let to_send = match status {
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
    };

    this.send(to_send)
}

// SAFETY: This is a raw FFI function called by our C++ code.
pub unsafe extern "C" fn cxx_async_future_poll<Output, Future>(
    this: &mut Future,
    result: *mut u8,
    waker_data: *const u8,
) -> u32
where
    Future: RustFuture<Output = Output>,
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

// SAFETY: This is a raw FFI function called by our C++ code.
//
// This needs an out pointer because of https://github.com/rust-lang/rust-bindgen/issues/778
pub unsafe extern "C" fn cxx_async_execlet_bundle<Future, Exec, Output>(
    out_bundle: *mut CxxAsyncExecletBundle<Future, Exec>,
) where
    Future: FutureWrap<Output = Output>,
    Exec: From<Execlet<Output>>,
    Output: Clone + Send + 'static,
{
    let (future, execlet) = Execlet::<Output>::bundle();
    let bundle = CxxAsyncExecletBundle {
        future: Future::from_infallible(future),
        execlet: Box::new(execlet.into()),
    };
    ptr::copy_nonoverlapping(&bundle, out_bundle, 1);
    mem::forget(bundle);
}

pub unsafe extern "C" fn cxx_async_execlet_submit<Output>(
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

pub unsafe extern "C" fn cxx_async_execlet_send<Output>(
    this: &Execlet<Output>,
    value: *const Output,
) where
    Output: Clone,
{
    let mut this = this.0.lock().unwrap();
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

// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_clone(address: *const ()) -> RawWaker {
    RawWaker::new(
        suspended_coroutine_clone(address as *mut () as *mut u8) as *mut () as *const (),
        &CXXASYNC_WAKER_VTABLE,
    )
}

// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake(address: *const ()) {
    suspended_coroutine_wake(address as *mut () as *mut u8)
}

// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_wake_by_ref(address: *const ()) {
    suspended_coroutine_wake_by_ref(address as *mut () as *mut u8)
}

// SAFETY: This is a raw FFI function called by the currently-running Rust executor.
unsafe fn rust_suspended_coroutine_drop(address: *const ()) {
    suspended_coroutine_drop(address as *mut () as *mut u8)
}
