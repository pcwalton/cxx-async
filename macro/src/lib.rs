/*
 * Copyright (c) Meta Platforms, Inc. and its affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/macro/src/lib.rs
//
//! The definition of the `#[bridge_future]` macro.
//!
//! Don't depend on this crate directly; just use the reexported macro in `cxx-async`.

use bitvec::prelude::{bitvec, Lsb0};
use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashMap;
use std::fmt::Write;
use std::iter;
use syn::parse::{Parse, ParseStream, Result as ParseResult};
use syn::{Fields, Ident, ItemStruct, Path, Token, Type};

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
///
/// If the future is inside a C++ namespace, add a `namespace = ...` attribute to the
/// `#[cxx_async::bridge_future]` attribute like so:
///
/// ```ignore
/// #[cxx::bridge]
/// #[namespace = mycompany::myproject]
/// mod ffi {
///     extern "Rust" {
///         type RustFutureStringNamespaced;
///     }
/// }
///
/// #[cxx_async::bridge_future(namespace = mycompany::myproject)]
/// struct RustFutureStringNamespaced(String);
/// ```
#[proc_macro_attribute]
pub fn bridge_future(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let AstPieces {
        future,
        output,
        drop_sender_glue,
        vtable_glue,
    } = AstPieces::from_token_streams(attribute, item);
    (quote! {
        /// A future shared between Rust and C++.
        pub struct #future {
            // FIXME(pcwalton): Unfortunately, as far as I can tell this has to be double-boxed
            // because we have to return these by value and we need the `CxxAsyncReceiver`
            // type to be Sized.
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

        // Define how to wrap concrete receivers in the future type we're defining.
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

        #[no_mangle]
        #[doc(hidden)]
        pub unsafe extern "C" fn #vtable_glue() -> *const ::cxx_async::CxxAsyncVtable {
            static VTABLE: ::cxx_async::CxxAsyncVtable = ::cxx_async::CxxAsyncVtable {
                channel: ::cxx_async::future_channel::<#future, #output> as *mut u8,
                sender_send: ::cxx_async::sender_future_send::<#output> as *mut u8,
                future_poll: ::cxx_async::future_poll::<#future, #output> as *mut u8,
            };
            return &VTABLE;
        }
    })
    .into()
}

/// Defines a C++ stream type that can be awaited from Rust.
///
/// The syntax to use is:
///
/// ```ignore
/// #[cxx_async::bridge_stream]
/// struct RustStreamString(String);
/// ```
///
/// Here, `RustStreamString` is the name of the stream type declared inside the
/// `extern Rust { ... }` block inside the `#[cxx::bridge]` module. `String` is the Rust type that
/// the stream yields when consumed; on the Rust side it will be automatically wrapped in a
/// `CxxAsyncResult`. On the C++ side it will be converted to the appropriate type, following the
/// `cxx` rules. Err returns are translated into C++ exceptions.
///
/// If the stream is inside a C++ namespace, add a `namespace = ...` attribute to the
/// `#[cxx_async::bridge_stream]` attribute like so:
///
/// ```ignore
/// #[cxx::bridge]
/// #[namespace = mycompany::myproject]
/// mod ffi {
///     extern "Rust" {
///         type RustStreamStringNamespaced;
///     }
/// }
///
/// #[cxx_async::bridge_stream(namespace = mycompany::myproject)]
/// struct RustStreamStringNamespaced(String);
/// ```
#[proc_macro_attribute]
pub fn bridge_stream(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let AstPieces {
        future: stream,
        output: item,
        drop_sender_glue,
        vtable_glue,
    } = AstPieces::from_token_streams(attribute, item);
    (quote! {
        /// A multi-shot stream shared between Rust and C++.
        pub struct #stream {
            // FIXME(pcwalton): Unfortunately, as far as I can tell this has to be double-boxed
            // because we have to return these by value and we need the `CxxAsyncReceiver` type
            // to be Sized.
            stream: ::futures::stream::BoxStream<'static, ::cxx_async::CxxAsyncResult<#item>>,
        }

        impl #stream {
            // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
            // 1. The struct does not implement Drop (other than for debugging, which doesn't
            // move the field).
            // 2. The struct doesn't implement Unpin.
            // 3. The struct isn't `repr(packed)`.
            ::cxx_async::unsafe_pinned!(stream: ::futures::stream::BoxStream<'static,
                ::cxx_async::CxxAsyncResult<#item>>);
        }

        // Define how to box up a future.
        impl ::cxx_async::IntoCxxAsyncStream for #stream {
            type Item = #item;
            fn fallible<Stm>(stream: Stm) -> Box<Self> where Stm: ::futures::stream::Stream<Item =
                    ::cxx_async::CxxAsyncResult<#item>> + Send + 'static {
                Box::new(#stream {
                    stream: Box::pin(stream),
                })
            }
        }

        // Implement the Rust Stream trait.
        impl ::futures::stream::Stream for #stream {
            type Item = ::cxx_async::CxxAsyncResult<#item>;
            fn poll_next(mut self: ::std::pin::Pin<&mut Self>, cx: &mut ::std::task::Context<'_>)
                    -> ::std::task::Poll<Option<Self::Item>> {
                ::std::pin::Pin::new(&mut self.stream).poll_next(cx)
            }
        }

        // Define how to wrap concrete receivers in the stream type we're defining.
        impl ::std::convert::From<::cxx_async::CxxAsyncReceiver<#item>> for #stream {
            fn from(receiver: ::cxx_async::CxxAsyncReceiver<#item>) -> Self {
                Self {
                    stream: Box::pin(receiver),
                }
            }
        }

        // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
        impl #stream {
            pub fn infallible<Stm>(stream: Stm) -> Box<Self>
                    where Stm: ::futures::stream::Stream<Item = #item> + Send + 'static {
                <#stream as ::cxx_async::IntoCxxAsyncStream>::infallible(stream)
            }

            pub fn fallible<Stm>(stream: Stm) -> Box<Self>
                    where Stm: ::futures::stream::Stream<Item =
                        ::cxx_async::CxxAsyncResult<#item>> + Send + 'static {
                <#stream as ::cxx_async::IntoCxxAsyncStream>::fallible(stream)
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
                Box<::cxx_async::CxxAsyncSender<#item>>) {
            ::cxx_async::drop_glue(ptr)
        }

        #[no_mangle]
        #[doc(hidden)]
        pub unsafe extern "C" fn #vtable_glue() -> *const ::cxx_async::CxxAsyncVtable {
            // TODO(pcwalton): Support C++ calling Rust Streams.
            static VTABLE: ::cxx_async::CxxAsyncVtable = ::cxx_async::CxxAsyncVtable {
                channel: ::cxx_async::stream_channel::<#stream, #item> as *mut u8,
                sender_send: ::cxx_async::sender_stream_send::<#item> as *mut u8,
                future_poll: ::std::ptr::null_mut(),
            };
            return &VTABLE;
        }
    })
    .into()
}

// AST pieces generated for the `#[bridge_future]` and `#[bridge_stream]` macros.
struct AstPieces {
    // The name of the future or stream type.
    future: Ident,
    // The output type of the future or the item type of the stream.
    output: Type,
    // The symbol name of the glue function that drops a sender.
    drop_sender_glue: Ident,
    // The symbol name of the future/stream vtable.
    vtable_glue: Ident,
}

impl AstPieces {
    // Parses the macro arguments and returns the pieces, panicking on error.
    fn from_token_streams(attribute: TokenStream, item: TokenStream) -> AstPieces {
        let namespace: NamespaceAttribute = match syn::parse(attribute) {
            Ok(namespace) => namespace,
            Err(_) => panic!("expected possible namespace attribute"),
        };

        let struct_item: ItemStruct = match syn::parse(item) {
            Ok(struct_item) => struct_item,
            Err(_) => panic!("expected struct"),
        };
        let output = match struct_item.fields {
            Fields::Unnamed(fields) => {
                if fields.unnamed.len() != 1 {
                    panic!("expected a tuple struct with a single field");
                }
                fields.unnamed.into_iter().next().unwrap().ty
            }
            Fields::Named { .. } | Fields::Unit => panic!("expected tuple struct"),
        };

        let future = struct_item.ident;
        let future_name_string = format!("{}", future);

        let drop_sender_glue = Ident::new(
            &mangle_drop_glue("RustSender", &future_name_string, &namespace.0),
            future.span(),
        );
        let vtable_glue = Ident::new(
            &mangle_vtable_glue(&future_name_string, &namespace.0),
            future.span(),
        );

        AstPieces {
            future,
            output,
            drop_sender_glue,
            vtable_glue,
        }
    }
}

mod keywords {
    use syn::custom_keyword;
    custom_keyword!(namespace);
}

struct NamespaceAttribute(Vec<String>);

impl Parse for NamespaceAttribute {
    fn parse(input: ParseStream) -> ParseResult<Self> {
        if input.is_empty() {
            return Ok(NamespaceAttribute(vec![]));
        }
        input.parse::<keywords::namespace>()?;
        input.parse::<Token![=]>()?;
        let path = input.call(Path::parse_mod_style)?;
        Ok(NamespaceAttribute(
            path.segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect(),
        ))
    }
}

fn mangle_drop_glue(name: &str, future: &str, namespace: &[String]) -> String {
    let mut tokens = vec![
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
    ];
    push_cxx_name_tokens_for_namespace(
        &mut tokens,
        namespace
            .iter()
            .map(|ident| &**ident)
            .chain(iter::once(future)),
    );
    tokens.extend_from_slice(&[
        CxxNameToken::EndTemplate,
        CxxNameToken::EndQName,
        CxxNameToken::EndTemplate,
        CxxNameToken::Name("drop"),
        CxxNameToken::EndQName,
        CxxNameToken::VoidArg,
    ]);
    mangle_cxx_name(&tokens)
}

fn mangle_vtable_glue(future_name_string: &str, namespace: &[String]) -> String {
    let mut tokens = vec![
        CxxNameToken::StartQName,
        CxxNameToken::Name("rust"),
        CxxNameToken::Name("async"),
        CxxNameToken::Name("FutureVtableProvider"),
        CxxNameToken::StartTemplate,
    ];
    push_cxx_name_tokens_for_namespace(
        &mut tokens,
        namespace
            .iter()
            .map(|ident| &**ident)
            .chain(iter::once(future_name_string)),
    );
    tokens.extend_from_slice(&[
        CxxNameToken::EndTemplate,
        CxxNameToken::Name("vtable"),
        CxxNameToken::EndQName,
        CxxNameToken::VoidArg,
    ]);
    mangle_cxx_name(&tokens)
}

#[derive(Clone, Copy)]
enum CxxNameToken<'a> {
    Name(&'a str),
    StartQName,
    EndQName,
    StartTemplate,
    EndTemplate,
    VoidArg,
}

fn push_cxx_name_tokens_for_namespace<'a, I>(tokens: &mut Vec<CxxNameToken<'a>>, name: I)
where
    I: Iterator<Item = &'a str> + Clone,
{
    let is_namespaced = name.clone().count() > 1;
    if is_namespaced {
        tokens.push(CxxNameToken::StartQName);
    }
    for segment in name {
        tokens.push(CxxNameToken::Name(segment));
    }
    if is_namespaced {
        tokens.push(CxxNameToken::EndQName);
    }
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

    // A stack of groups. True if substitution is eligible in this context; false if it is not.
    let mut substitution_eligible = bitvec![1];

    for token in tokens {
        match *token {
            CxxNameToken::Name(name) if *substitution_eligible.last().unwrap() => {
                match substitutions.get(name) {
                    None => {
                        substitutions.insert(name, substitution_count);
                        substitution_count += 1;
                        write!(&mut string, "{}{}", name.len(), name).unwrap();
                        *substitution_eligible.last_mut().unwrap() = false;
                    }
                    Some(&0) => write!(&mut string, "S_").unwrap(),
                    Some(_) => unimplemented!(),
                }
            }
            CxxNameToken::Name(name) => {
                write!(&mut string, "{}{}", name.len(), name).unwrap();
            }
            CxxNameToken::StartQName => {
                substitution_eligible.push(true);
                string.push('N');
            }
            CxxNameToken::StartTemplate => {
                substitution_eligible.push(true);
                string.push('I')
            }
            CxxNameToken::EndQName | CxxNameToken::EndTemplate => {
                substitution_eligible.pop();
                string.push('E');
            }
            CxxNameToken::VoidArg => string.push('v'),
        }
    }
    string
}
