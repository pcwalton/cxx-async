// cxx-async2/macro/src/lib.rs
//
//! The definition of the `#[bridge_future]` macro.
//!
//! Don't depend on this crate directly; just use the reexported macro in `cxx-async`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Fields, Ident, ItemStruct};

#[proc_macro_attribute]
pub fn bridge_future(_: TokenStream, item: TokenStream) -> TokenStream {
    let struct_item: ItemStruct = match syn::parse(item) {
        Ok(struct_item) => struct_item,
        Err(_) => panic!("expected struct"),
    };
    let output = match struct_item.fields {
        Fields::Unnamed(fields) => {
            if fields.unnamed.len() != 1 {
                panic!("expected a tuple struct with a single field");
            }
            fields.unnamed.into_iter().nth(0).unwrap().ty
        }
        Fields::Named { .. } | Fields::Unit => panic!("expected tuple struct"),
    };

    let future = struct_item.ident;
    let sender = Ident::new(&format!("{}Sender", future), future.span());
    let receiver = Ident::new(&format!("{}Receiver", future), future.span());
    let execlet = Ident::new(&format!("{}Execlet", future), future.span());
    let drop_sender_glue = Ident::new(&format!("cxxasync_drop_box_rust_sender_{}",
        future), future.span());
    let drop_execlet_glue = Ident::new(&format!("cxxasync_drop_box_rust_execlet_{}",
        future), future.span());
    let vtable = Ident::new(&format!("cxxasync_vtable_{}", future), future.span());

    quote! {
        /// A future shared between Rust and C++.
        pub struct #future {
            // FIXME(pcwalton): Unfortunately, as far as I can tell this has to be double-boxed
            // because we need the `RustFuture` type to be Sized.
            future: ::futures::future::BoxFuture<'static, ::cxx_async2::CxxAsyncResult<#output>>,
        }

        // The wrapper for the sending end.
        #[repr(transparent)]
        #[doc(hidden)]
        pub struct #sender(
            Option<::futures::channel::oneshot::Sender<::cxx_async2::CxxAsyncResult<#output>>>);

        // A type alias for the receiving end (i.e. the concrete future type).
        type #receiver = ::cxx_async2::CxxAsyncReceiver<#output>;

        // The wrapped execlet for this future.
        #[repr(transparent)]
        pub struct #execlet(::cxx_async2::Execlet<#output>);

        impl #future {
            // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
            // 1. The struct does not implement Drop (other than for debugging, which doesn't
            // move the field).
            // 2. The struct doesn't implement Unpin.
            // 3. The struct isn't `repr(packed)`.
            ::cxx_async2::unsafe_pinned!(future: ::futures::future::BoxFuture<'static,
                ::cxx_async2::CxxAsyncResult<#output>>);
        }

        // Define how to box up a future.
        impl ::cxx_async2::IntoCxxAsyncFuture for #future {
            type Output = #output;
            fn fallible<Fut>(future: Fut) -> Box<Self> where Fut: ::std::future::Future<Output =
                    ::cxx_async2::CxxAsyncResult<#output>> + Send + 'static {
                Box::new(#future {
                    future: Box::pin(future),
                })
            }
        }

        // Implement the Rust Future trait.
        impl ::std::future::Future for #future {
            type Output = ::cxx_async2::CxxAsyncResult<#output>;
            fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                    -> ::std::task::Poll<Self::Output> {
                self.future().poll(cx)
            }
        }

        // Implement the CxxAsyncFuture trait that doesn't require pinning.
        impl ::cxx_async2::CxxAsyncFuture for #future {
            type Output = #output;
            fn poll(&mut self, context: &mut ::std::task::Context)
                    -> ::std::task::Poll<::cxx_async2::CxxAsyncResult<Self::Output>> {
                ::std::future::Future::poll(::std::pin::Pin::new(&mut self.future), context)
            }
        }

        // Implement sending for the Sender trait.
        //
        // FIXME(pcwalton): Not sure if we need this?
        impl ::cxx_async2::RustSender for #sender {
            type Output = #output;
            fn send(&mut self, value: ::cxx_async2::CxxAsyncResult<#output>) {
                self.0.take().unwrap().send(value).unwrap()
            }
        }

        // Define how to wrap concrete receivers in the `RustFuture` type.
        impl ::std::convert::From<#receiver> for #future {
            fn from(receiver: #receiver) -> Self {
                Self {
                    future: Box::pin(receiver),
                }
            }
        }

        // Define how to wrap raw oneshot senders in the `RustSender` type.
        impl ::std::convert::From<::futures::channel::oneshot::Sender<
                ::cxx_async2::CxxAsyncResult<#output>>> for #sender {
            fn from(sender:
                    ::futures::channel::oneshot::Sender<::cxx_async2::CxxAsyncResult<#output>>) ->
                    Self {
                Self(Some(sender))
            }
        }

        // Define how to wrap raw Execlets in the `RustExeclet` type we just defined.
        impl ::std::convert::From<::cxx_async2::Execlet<#output>> for #execlet {
            fn from(execlet: ::cxx_async2::Execlet<#output>) -> Self {
                Self(execlet)
            }
        }

        // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
        impl #future {
            pub fn infallible<Fut>(future: Fut) -> Box<Self>
                    where Fut: ::std::future::Future<Output = #output> + Send + 'static {
                <#future as ::cxx_async2::IntoCxxAsyncFuture>::infallible(future)
            }

            pub fn fallible<Fut>(future: Fut) -> Box<Self>
                    where Fut: ::std::future::Future<Output =
                        ::cxx_async2::CxxAsyncResult<#output>> + Send + 'static {
                <#future as ::cxx_async2::IntoCxxAsyncFuture>::fallible(future)
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
        pub unsafe extern "C" fn #drop_sender_glue(ptr: *mut Box<#sender>) {
            let mut boxed: ::std::mem::MaybeUninit<Box<#sender>> =
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
        pub unsafe extern "C" fn #drop_execlet_glue(ptr: *mut Box<#execlet>) {
            let mut boxed: ::std::mem::MaybeUninit<Box<#execlet>> =
                ::std::mem::MaybeUninit::uninit();
            ::std::ptr::copy_nonoverlapping(ptr, boxed.as_mut_ptr(), 1);
            drop(boxed.assume_init());
        }

        #[doc(hidden)]
        #[no_mangle]
        pub static #vtable: ::cxx_async2::CxxAsyncVtable = ::cxx_async2::CxxAsyncVtable {
            channel: ::cxx_async2::channel::<#future, #sender, #receiver, #output> as *mut u8,
            sender_send: ::cxx_async2::sender_send::<#sender, #output> as *mut u8,
            future_poll: ::cxx_async2::future_poll::<#output, #future> as *mut u8,
            execlet: ::cxx_async2::execlet_bundle::<#future, #execlet, #output> as *mut u8,
            execlet_submit: ::cxx_async2::execlet_submit::<#output> as *mut u8,
            execlet_send: ::cxx_async2::execlet_send::<#output> as *mut u8,
        };
    }.into()
}
