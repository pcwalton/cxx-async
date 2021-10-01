// cxx-async/macro/src/lib.rs
//
//! The definition of the `#[bridge_future]` macro.
//!
//! Don't depend on this crate directly; just use the reexported macro in `cxx-async`.

use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashMap;
use std::fmt::Write;
use syn::{Fields, Ident, ItemStruct};

/// Defines a future type that can be awaited from both Rust and C++.
///
/// The syntax to use is:
///
/// ```ignore
/// #[cxx_async::bridge_future]
/// struct RustFutureString(String);
/// ```
///
/// Here, `RustFutureString` is the name of the future type declared inside the
/// `extern Rust { ... }` block inside the `#[cxx::bridge]` module. `String` is the Rust type that
/// the future yields when awaited; on the Rust side it will be automatically wrapped in a
/// `CxxAsyncResult`. On the C++ side it will be converted to the appropriate type, following the
/// `cxx` rules. Err returns are translated into C++ exceptions.
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
    let future_name_string = format!("{}", future);

    let drop_sender_glue = Ident::new(
        &mangle_drop_glue("RustSender", &future_name_string),
        future.span(),
    );
    let drop_execlet_glue = Ident::new(
        &mangle_drop_glue("RustExeclet", &future_name_string),
        future.span(),
    );
    let vtable_glue = Ident::new(
        &mangle_cxx_name(&[
            CxxNameToken::StartQName,
            CxxNameToken::Name("rust"),
            CxxNameToken::Name("async"),
            CxxNameToken::Name("FutureVtableProvider"),
            CxxNameToken::StartTemplate,
            CxxNameToken::Name(&future_name_string),
            CxxNameToken::EndTemplate,
            CxxNameToken::Name("vtable"),
            CxxNameToken::EndQName,
            CxxNameToken::VoidArg,
        ]),
        future.span(),
    );

    quote! {
        /// A future shared between Rust and C++.
        pub struct #future {
            // FIXME(pcwalton): Unfortunately, as far as I can tell this has to be double-boxed
            // because we need the `RustFuture` type to be Sized.
            future: ::futures::future::BoxFuture<'static, ::cxx_async::CxxAsyncResult<#output>>,
        }

        impl #future {
            // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
            // 1. The struct does not implement Drop (other than for debugging, which doesn't
            // move the field).
            // 2. The struct doesn't implement Unpin.
            // 3. The struct isn't `repr(packed)`.
            ::cxx_async::unsafe_pinned!(future: ::futures::future::BoxFuture<'static,
                ::cxx_async::CxxAsyncResult<#output>>);
        }

        // Define how to box up a future.
        impl ::cxx_async::IntoCxxAsyncFuture for #future {
            type Output = #output;
            fn fallible<Fut>(future: Fut) -> Box<Self> where Fut: ::std::future::Future<Output =
                    ::cxx_async::CxxAsyncResult<#output>> + Send + 'static {
                Box::new(#future {
                    future: Box::pin(future),
                })
            }
        }

        // Implement the Rust Future trait.
        impl ::std::future::Future for #future {
            type Output = ::cxx_async::CxxAsyncResult<#output>;
            fn poll(self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                    -> ::std::task::Poll<Self::Output> {
                self.future().poll(cx)
            }
        }

        // Define how to wrap concrete receivers in the `RustFuture` type.
        impl ::std::convert::From<::cxx_async::CxxAsyncReceiver<#output>> for #future {
            fn from(receiver: ::cxx_async::CxxAsyncReceiver<#output>) -> Self {
                Self {
                    future: Box::pin(receiver),
                }
            }
        }

        // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
        impl #future {
            pub fn infallible<Fut>(future: Fut) -> Box<Self>
                    where Fut: ::std::future::Future<Output = #output> + Send + 'static {
                <#future as ::cxx_async::IntoCxxAsyncFuture>::infallible(future)
            }

            pub fn fallible<Fut>(future: Fut) -> Box<Self>
                    where Fut: ::std::future::Future<Output =
                        ::cxx_async::CxxAsyncResult<#output>> + Send + 'static {
                <#future as ::cxx_async::IntoCxxAsyncFuture>::fallible(future)
            }
        }

        // The C++ bridge calls this to destroy a sender.
        //
        // I'm not sure if this can ever legitimately happen, but C++ wants to link to this
        // function anyway, so let's provide it.
        //
        // SAFETY: This is a raw FFI function called by `cxx`. `cxx` ensures that `ptr` is a
        // valid Box.
        #[no_mangle]
        #[doc(hidden)]
        pub unsafe extern "C" fn #drop_sender_glue(ptr: *mut
                Box<::cxx_async::CxxAsyncSender<#output>>) {
            ::cxx_async::drop_glue(ptr)
        }

        // The C++ bridge calls this to destroy an `Execlet`.
        //
        // SAFETY: This is a raw FFI function called by `cxx`. `cxx` ensures that `ptr` is a
        // valid Box.
        #[no_mangle]
        #[doc(hidden)]
        pub unsafe extern "C" fn #drop_execlet_glue(ptr: *mut Box<::cxx_async::Execlet<#output>>) {
            ::cxx_async::drop_glue(ptr)
        }

        #[no_mangle]
        #[doc(hidden)]
        pub unsafe extern "C" fn #vtable_glue() -> *const ::cxx_async::CxxAsyncVtable {
            static VTABLE: ::cxx_async::CxxAsyncVtable = ::cxx_async::CxxAsyncVtable {
                channel: ::cxx_async::channel::<#future, #output> as *mut u8,
                sender_send: ::cxx_async::sender_send::<#output> as *mut u8,
                future_poll: ::cxx_async::future_poll::<#future, #output> as *mut u8,
                execlet: ::cxx_async::execlet_bundle::<#future, #output> as *mut u8,
                execlet_submit: ::cxx_async::execlet_submit::<#output> as *mut u8,
                execlet_send: ::cxx_async::execlet_send::<#output> as *mut u8,
            };
            return &VTABLE;
        }
    }
    .into()
}

fn mangle_drop_glue(name: &str, future: &str) -> String {
    mangle_cxx_name(&[
        CxxNameToken::StartQName,
        CxxNameToken::Name("rust"),
        CxxNameToken::Name("cxxbridge1"),
        CxxNameToken::Name("Box"),
        CxxNameToken::StartTemplate,
        CxxNameToken::StartQName,
        CxxNameToken::Name("rust"),
        CxxNameToken::Name("async"),
        CxxNameToken::Name(name),
        CxxNameToken::StartTemplate,
        CxxNameToken::Name(future),
        CxxNameToken::EndTemplate,
        CxxNameToken::EndQName,
        CxxNameToken::EndTemplate,
        CxxNameToken::Name("drop"),
        CxxNameToken::EndQName,
        CxxNameToken::VoidArg,
    ])
}

enum CxxNameToken<'a> {
    Name(&'a str),
    StartQName,
    EndQName,
    StartTemplate,
    EndTemplate,
    VoidArg,
}

// Mangles a C++ name.
//
// This isn't a general C++ name mangling routine; it's just enough for what we need to
// emit.
//
// TODO(pcwalton): MSVC mangling.
fn mangle_cxx_name(tokens: &[CxxNameToken]) -> String {
    let mut string = "_Z".to_owned();
    let (mut substitutions, mut substitution_count) = (HashMap::new(), 0u32);

    for token in tokens {
        match *token {
            CxxNameToken::Name(name) => match substitutions.get(name) {
                None => {
                    substitutions.insert(name, substitution_count);
                    substitution_count += 1;
                    write!(&mut string, "{}{}", name.len(), name).unwrap();
                }
                Some(&0) => write!(&mut string, "S_").unwrap(),
                Some(_) => unimplemented!(),
            },
            CxxNameToken::StartQName => string.push_str("N"),
            CxxNameToken::EndQName => string.push_str("E"),
            CxxNameToken::StartTemplate => string.push_str("I"),
            CxxNameToken::EndTemplate => string.push_str("E"),
            CxxNameToken::VoidArg => string.push_str("v"),
        }
    }
    string
}
