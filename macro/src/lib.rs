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
//! The definition of the `#[bridge]` macro.
//!
//! Don't depend on this crate directly; just use the reexported macro in `cxx-async`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream, Result as ParseResult};
use syn::{Ident, ImplItem, Lit, LitStr, Path, Token, Type, ItemImpl, Visibility, TypePath};

/// Defines a future or stream type that can be awaited from both Rust and C++.
///
/// The syntax to use is:
///
/// ```ignore
/// #[cxx_async::bridge]
/// unsafe impl Future for RustFutureString {
///     type Output = String;
/// }
/// ```
///
/// Here, `RustFutureString` is the name of the future type declared inside the
/// `extern Rust { ... }` block inside the `#[cxx::bridge]` module. `String` is the Rust type that
/// the future yields when awaited; on the Rust side it will be automatically wrapped in a
/// `CxxAsyncResult`. On the C++ side it will be converted to the appropriate type, following the
/// `cxx` rules. Err returns are translated into C++ exceptions.
///
/// If the future is inside a C++ namespace, add a `namespace = ...` attribute to the
/// `#[cxx_async::bridge]` attribute like so:
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
/// #[cxx_async::bridge(namespace = mycompany::myproject)]
/// unsafe impl Future for RustFutureStringNamespaced {
///     type Output = String;
/// }
/// ```
#[proc_macro_attribute]
pub fn bridge(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let pieces = AstPieces::from_token_streams(attribute, item);
    match pieces.bridge_trait {
        BridgeTrait::Future => bridge_future(pieces),
        BridgeTrait::Stream => bridge_stream(pieces),
    }
}

fn bridge_future(pieces: AstPieces) -> TokenStream {
    let AstPieces {
        bridge_trait: _,
        future,
        qualified_name,
        output,
        trait_path,
        vtable_glue_ident,
        vtable_glue_link_name,
    } = pieces;
    (quote! {
        /// A future shared between Rust and C++.
        #[repr(transparent)]
        pub struct #future {
            future: ::cxx_async::private::BoxFuture<'static, ::cxx_async::CxxAsyncResult<#output>>,
        }

        impl #future {
            // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
            // 1. The struct doesn't move fields in Drop. Our hogging the Drop implementation
            //    ensures this.
            // 2. The inner field implements Unpin. We ensure this in the `assert_field_is_unpin`
            //    method.
            // 3. The struct isn't `repr(packed)`. We define the struct and don't have this
            //    attribute.
            ::cxx_async::unsafe_pinned!(future: ::cxx_async::private::BoxFuture<'static,
                ::cxx_async::CxxAsyncResult<#output>>);

            #[doc(hidden)]
            fn assert_field_is_unpin() {
                fn check<T>() where T: Unpin {}
                check::<#output>()
            }
        }

        // Define a Drop implementation so that end users don't. If end users are allowed to define
        // Drop, that could make our use of `unsafe_pinned!` unsafe.
        impl Drop for #future {
            fn drop(&mut self) {}
        }

        // Define how to box up a future.
        impl ::cxx_async::IntoCxxAsyncFuture for #future {
            type Output = #output;
            fn fallible<Fut>(future: Fut) -> Self where Fut: ::std::future::Future<Output =
                    ::cxx_async::CxxAsyncResult<#output>> + Send + 'static {
                #future {
                    future: Box::pin(future),
                }
            }
        }

        // Implement the Rust Future trait.
        impl #trait_path for #future {
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

        // Make sure that the future type can be returned by value.
        // See: https://github.com/dtolnay/cxx/pull/672
        unsafe impl ::cxx::ExternType for #future {
            type Id = ::cxx::type_id!(#qualified_name);
            type Kind = ::cxx::kind::Trivial;
        }

        // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
        impl #future {
            pub fn infallible<Fut>(future: Fut) -> Self
                    where Fut: ::std::future::Future<Output = #output> + Send + 'static {
                <#future as ::cxx_async::IntoCxxAsyncFuture>::infallible(future)
            }

            pub fn fallible<Fut>(future: Fut) -> Self
                    where Fut: ::std::future::Future<Output =
                        ::cxx_async::CxxAsyncResult<#output>> + Send + 'static {
                <#future as ::cxx_async::IntoCxxAsyncFuture>::fallible(future)
            }
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #[export_name = #vtable_glue_link_name]
        pub unsafe extern "C" fn #vtable_glue_ident() -> *const ::cxx_async::CxxAsyncVtable {
            static VTABLE: ::cxx_async::CxxAsyncVtable = ::cxx_async::CxxAsyncVtable {
                channel: ::cxx_async::future_channel::<#future, #output> as *mut u8,
                sender_send: ::cxx_async::sender_future_send::<#output> as *mut u8,
                sender_drop: ::cxx_async::sender_drop::<#output> as *mut u8,
                future_poll: ::cxx_async::future_poll::<#future, #output> as *mut u8,
                future_drop: ::cxx_async::future_drop::<#future> as *mut u8,
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
/// unsafe impl Stream for RustStreamString {
///     type Item = String;
/// }
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
/// unsafe impl Stream for RustStreamStringNamespaced {
///     type Item = String;
/// }
/// ```
fn bridge_stream(pieces: AstPieces) -> TokenStream {
    let AstPieces {
        bridge_trait: _,
        future: stream,
        qualified_name,
        output: item,
        trait_path,
        vtable_glue_ident,
        vtable_glue_link_name,
    } = pieces;
    (quote! {
        /// A multi-shot stream shared between Rust and C++.
        #[repr(transparent)]
        pub struct #stream {
            stream: ::cxx_async::private::BoxStream<'static, ::cxx_async::CxxAsyncResult<#item>>,
        }

        impl #stream {
            // SAFETY: See: https://docs.rs/pin-utils/0.1.0/pin_utils/macro.unsafe_pinned.html
            // 1. The struct doesn't move fields in Drop. Our hogging the Drop implementation
            //    ensures this.
            // 2. The inner field implements Unpin. We ensure this in the `assert_field_is_unpin`
            //    method.
            // 3. The struct isn't `repr(packed)`. We define the struct and don't have this
            //    attribute.
            ::cxx_async::unsafe_pinned!(stream: ::cxx_async::private::BoxStream<'static,
                ::cxx_async::CxxAsyncResult<#item>>);

            #[doc(hidden)]
            fn assert_field_is_unpin() {
                fn check<T>() where T: Unpin {}
                check::<#item>()
            }
        }

        // Define a Drop implementation so that end users don't. If end users are allowed to define
        // Drop, that could make our use of `unsafe_pinned!` unsafe.
        impl Drop for #stream {
            fn drop(&mut self) {}
        }

        // Define how to box up a future.
        impl ::cxx_async::IntoCxxAsyncStream for #stream {
            type Item = #item;
            fn fallible<Stm>(stream: Stm) -> Self where Stm: ::cxx_async::private::Stream<Item =
                    ::cxx_async::CxxAsyncResult<#item>> + Send + 'static {
                #stream {
                    stream: Box::pin(stream),
                }
            }
        }

        // Implement the Rust Stream trait.
        impl #trait_path for #stream {
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

        // Make sure that the stream type can be returned by value.
        // See: https://github.com/dtolnay/cxx/pull/672
        unsafe impl ::cxx::ExternType for #stream {
            type Id = ::cxx::type_id!(#qualified_name);
            type Kind = ::cxx::kind::Trivial;
        }

        // Convenience wrappers so that client code doesn't have to import `IntoCxxAsyncFuture`.
        impl #stream {
            pub fn infallible<Stm>(stream: Stm) -> Self
                    where Stm: ::cxx_async::private::Stream<Item = #item> + Send + 'static {
                <#stream as ::cxx_async::IntoCxxAsyncStream>::infallible(stream)
            }

            pub fn fallible<Stm>(stream: Stm) -> Self
                    where Stm: ::cxx_async::private::Stream<Item =
                        ::cxx_async::CxxAsyncResult<#item>> + Send + 'static {
                <#stream as ::cxx_async::IntoCxxAsyncStream>::fallible(stream)
            }
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #[export_name = #vtable_glue_link_name]
        pub unsafe extern "C" fn #vtable_glue_ident() -> *const ::cxx_async::CxxAsyncVtable {
            // TODO(pcwalton): Support C++ calling Rust Streams.
            static VTABLE: ::cxx_async::CxxAsyncVtable = ::cxx_async::CxxAsyncVtable {
                channel: ::cxx_async::stream_channel::<#stream, #item> as *mut u8,
                sender_send: ::cxx_async::sender_stream_send::<#item> as *mut u8,
                sender_drop: ::cxx_async::sender_drop::<#item> as *mut u8,
                future_poll: ::std::ptr::null_mut(),
                future_drop: ::cxx_async::future_drop::<#stream> as *mut u8,
            };
            return &VTABLE;
        }
    })
    .into()
}

// Whether the programmer is implementing `Future` or `Stream`.
enum BridgeTrait {
    Future,
    Stream,
}

// AST pieces generated for the `#[bridge]` macro.
struct AstPieces {
    // Whether the programmer is implementing `Future` or `Stream`.
    bridge_trait: BridgeTrait,
    // The name of the future or stream type.
    future: Ident,
    // The fully-qualified name (i.e. including C++ namespace if any) of the future or stream type,
    // as a quoted string.
    qualified_name: Lit,
    // The output type of the future or the item type of the stream.
    output: Type,
    // The path to the trait being implemented, which must be `std::future::Future` or
    // `futures::Stream`.
    trait_path: Path,
    // The internal Rust symbol name of the future/stream vtable.
    vtable_glue_ident: Ident,
    // The external C++ link name of the future/stream vtable.
    vtable_glue_link_name: String,
}

impl AstPieces {
    // Parses the macro arguments and returns the pieces, panicking on error.
    fn from_token_streams(attribute: TokenStream, item: TokenStream) -> AstPieces {
        let namespace: NamespaceAttribute = match syn::parse(attribute) {
            Ok(namespace) => namespace,
            Err(_) => panic!("expected possible namespace attribute"),
        };

        let impl_item: ItemImpl = match syn::parse(item) {
            Ok(impl_item) => impl_item,
            Err(e) => panic!("expected implementation of `Future` or `Stream`: {:?}", e),
        };
        if impl_item.unsafety.is_none() {
            panic!("implementation must be marked `unsafe`")
        }
        if impl_item.defaultness.is_some() {
            panic!("implementation must not be marked default")
        }
        if !impl_item.generics.params.is_empty() || impl_item.generics.where_clause.is_some() {
            panic!("generic bridged futures are unsupported")
        }

        // We don't check to make sure that `path` is `std::future::Future` or `futures::Stream`
        // here, even though that's ultimately a requirement, because we would have to perform name
        // resolution here in the macro to do that. Instead, we take advantage of the fact that
        // we're going to be generating an implementation of the appropriate trait anyway and simply
        // supply whatever the user wrote as the name of the trait to be implemented in our final
        // macro expansion. That way, the Rust compiler ends up checking that the trait that the
        // user wrote is the right one.
        let trait_path = match impl_item.trait_ {
            Some((None, path, _)) => path,
            _ => panic!("must implement the `Future` or `Stream` trait"),
        };

        if impl_item.items.len() != 1 {
            panic!("expected implementation to contain a single item, `type Output = ...`")
        }
        let (bridge_trait, output);
        match impl_item.items[0] {
            ImplItem::Type(ref impl_type) => {
                if !impl_type.attrs.is_empty() {
                    panic!("attributes on the `type Output = ...` declaration are not supported");
                }
                match impl_type.vis {
                    Visibility::Inherited => {}
                    _ => {
                        panic!("`pub` or `crate` visibility modifiers on the `type Output = ...` \
                            declaration are not supported")
                    }
                }
                if impl_type.defaultness.is_some() {
                    panic!("`default` specifier on the `type Output = ...` declaration is not \
                        supported")
                }
                if !impl_type.generics.params.is_empty() {
                    panic!("generics on the `type Output = ...` declaration are not supported")
                }

                // We use the name of the associated type to disambiguate between a Future and a
                // Stream implementation.
                bridge_trait = if impl_type.ident == "Output" {
                    BridgeTrait::Future
                } else if impl_type.ident == "Item" {
                    BridgeTrait::Stream
                } else {
                    panic!("implementation must contain an associated type definition named \
                        `Output` or `Item`")
                };
                output = impl_type.ty.clone();
            }
            _ => {
                panic!("expected implementation to contain a single item, `type Output = ...` \
                    or `type Stream = ...`")
            }
        };

        let future = match *impl_item.self_ty {
            Type::Path(TypePath { qself: None, path }) => {
                match path.get_ident() {
                    None => panic!("expected `impl` declaration to implement a single type"),
                    Some(path) => (*path).clone(),
                }
            }
            _ => panic!("expected `impl` declaration to implement a single type"),
        };

        let qualified_name = Lit::Str(LitStr::new(
            &format!(
                "{}{}",
                namespace
                    .0
                    .iter()
                    .map(|piece| format!("{}::", piece))
                    .collect::<String>(),
                future
            ),
            future.span(),
        ));

        let vtable_glue_ident = Ident::new(
            &format!(
                "cxxasync_{}{}_vtable",
                namespace
                    .0
                    .iter()
                    .map(|piece| format!("{}_", piece))
                    .collect::<String>(),
                future
            ),
            future.span(),
        );
        let vtable_glue_link_name = format!(
            "cxxasync_{}{}_vtable",
            namespace
                .0
                .iter()
                .map(|piece| format!("{}$", piece))
                .collect::<String>(),
            future
        );

        AstPieces {
            bridge_trait,
            future,
            qualified_name,
            output,
            trait_path,
            vtable_glue_ident,
            vtable_glue_link_name,
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
