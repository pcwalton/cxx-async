// cxx-async2/src/main.rs

extern crate link_cplusplus;

use crate::ffi::{
    suspended_coroutine_clone, suspended_coroutine_drop, suspended_coroutine_wake,
    suspended_coroutine_wake_by_ref,
};
use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::task::{RawWaker, RawWakerVTable};

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

#[macro_export]
macro_rules! define_cxx_future {
    ($name:ident, $output:ty) => {
        ::cxx_async2::paste! {
            pub struct [<RustFuture $name>] {
                future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>,
            }

            pub struct [<RustSender $name>](Option<::futures::channel::oneshot::Sender<
                ::cxx_async2::CxxAsyncResult<$output>>>);

            struct [<RustReceiver $name>](::futures::channel::oneshot::Receiver<
                ::cxx_async2::CxxAsyncResult<$output>>);

            impl [<RustFuture $name>] {
                // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
                // (1) The struct does not implement Drop (other than for debugging, which doesn't
                // move the field).
                // (2) The struct doesn't implement Unpin.
                // (3) The struct isn't `repr(packed)`.
                ::cxx_async2::unsafe_pinned!(future: ::futures::future::BoxFuture<'static,
                    ::cxx_async2::CxxAsyncResult<$output>>);

                fn from(future: impl ::std::future::Future<Output = $output> + Send + 'static)
                        -> Box<Self> {
                    return Self::from_fallible(wrapper(future));

                    async fn wrapper(
                        future: impl ::std::future::Future<Output = $output> + Send + 'static,
                    ) -> ::cxx_async2::CxxAsyncResult<$output> {
                        Ok(future.await)
                    }
                }

                fn from_fallible(
                    future: impl ::std::future::Future<Output =
                        ::cxx_async2::CxxAsyncResult<$output>> + Send + 'static,
                ) -> Box<Self> {
                    Box::new([<RustFuture $name>] {
                        future: Box::pin(future),
                    })
                }

                unsafe fn channel(&self, _: *const $output) -> [<RustOneshot $name>] {
                    let (sender, receiver) = ::futures::channel::oneshot::channel();
                    [<RustOneshot $name>] {
                        sender: Box::new([<RustSender $name>](Some(sender))),
                        future: Box::new([<RustFuture $name>] {
                            future: Box::pin([<RustReceiver $name>](receiver)),
                        }),
                    }
                }

                unsafe fn poll(&mut self, result: *mut u8, waker_data: *const u8) -> u32 {
                    let waker = ::std::task::Waker::from_raw(::std::task::RawWaker::new(
                        waker_data as *const (),
                        &::cxx_async2::CXXASYNC_WAKER_VTABLE,
                    ));
                    let mut context = ::std::task::Context::from_waker(&waker);
                    match ::std::future::Future::poll(::std::pin::Pin::new(&mut self.future),
                            &mut context) {
                        ::std::task::Poll::Ready(Ok(value)) => {
                            ::std::ptr::copy_nonoverlapping(&value, result as *mut $output, 1);
                            ::std::mem::forget(value);
                            ::cxx_async2::FUTURE_STATUS_COMPLETE
                        }
                        ::std::task::Poll::Ready(Err(error)) => {
                            let error = error.what().to_owned();
                            ::std::ptr::copy_nonoverlapping(&error, result as *mut String, 1);
                            ::std::mem::forget(error);
                            ::cxx_async2::FUTURE_STATUS_ERROR
                        }
                        ::std::task::Poll::Pending => ::cxx_async2::FUTURE_STATUS_PENDING,
                    }
                }
            }

            impl [<RustSender $name>] {
                // Takes ownership of the value. The caller must not call its destructor.
                unsafe fn send(&mut self, status: u32, value: *const u8) {
                    let to_send = match status {
                        ::cxx_async2::FUTURE_STATUS_COMPLETE => {
                            let mut staging: ::std::mem::MaybeUninit<$output> =
                                ::std::mem::MaybeUninit::uninit();
                            ::std::ptr::copy_nonoverlapping(
                                value as *const $output,
                                staging.as_mut_ptr(),
                                1);
                            Ok(staging.assume_init())
                        }
                        ::cxx_async2::FUTURE_STATUS_ERROR => {
                            let string = ::std::ffi::CStr::from_ptr(value as *const i8);
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

            impl ::std::future::Future for [<RustFuture $name>] {
                type Output = ::cxx_async2::CxxAsyncResult<$output>;
                fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                        -> ::std::task::Poll<Self::Output> {
                    self.future().poll(cx)
                }
            }

            impl ::std::future::Future for [<RustReceiver $name>] {
                type Output = ::cxx_async2::CxxAsyncResult<$output>;
                fn poll(mut self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                        -> ::std::task::Poll<Self::Output> {
                    match ::std::future::Future::poll(::std::pin::Pin::new(&mut self.0), cx) {
                        ::std::task::Poll::Pending => ::std::task::Poll::Pending,
                        ::std::task::Poll::Ready(Ok(value)) => ::std::task::Poll::Ready(value),
                        ::std::task::Poll::Ready(Err(Canceled)) => {
                            ::std::task::Poll::Ready(Err(CxxAsyncException::new(
                                "Canceled".to_owned().into_boxed_str(),
                            )))
                        }
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
