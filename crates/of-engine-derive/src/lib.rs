//! `#[derive(StateClone)]` (ARCHITECTURE §8.1, review 1 #3).
//!
//! `#[derive(Clone)]` never generates `clone_from`, so the default `clone_from` is
//! `*self = src.clone()`, which reallocates every buffer. Pooled forks
//! (`src.clone_into(&mut pooled)` = `pooled.clone_from(&src)`) must reuse memory, so this
//! derive generates:
//! - `Clone::clone`: field-wise `Clone::clone` (like `derive(Clone)`);
//! - `Clone::clone_from`: field-wise `Clone::clone_from` for structs; for enums, field-wise
//!   when both sides are the same variant, else `*self = src.clone()`;
//! - `impl of_engine::plain::PlainState` for the type, plus a compile-time assertion that
//!   every field type is `PlainState`. `Rc`, `RefCell`, `Cell`, `Box<T>`/`Box<dyn ..>` and
//!   `std` hash containers are deliberately not `PlainState`, so putting one into a state
//!   type is a compile error (the "no Rc/RefCell/Box<dyn> in State" rule, §8.1 item 2).
//!
//! - `impl of_engine::canon::Refs` (ARCHITECTURE §3.11, §9): visits every field **in
//!   declaration order** (= snapshot dump order, then non-snapshot fields), so a state type
//!   can never miss a handle (a missed handle is a GC use-after-free, FILE_MAP §5 rule 7).
//!   Field attributes:
//!   - `#[refs(skip)]`: never visited (e.g. an arena whose contents the GC/exporter walk
//!     themselves);
//!   - `#[refs(gc_only)]`: visited only when `!v.canon_pass()` (non-snapshot fields that hold
//!     handles: memos, lookup maps; the CS drain loop must not see them, SPEC §1.2.1);
//!   - `#[refs(chain_version)]`: a `u32` that names a water chain slot →
//!     `v.visit_chain_version(x)`.
//!
//!   Container attribute `#[refs(manual)]`: no `Refs` impl is generated (write it by hand).
//!
//! Generic type parameters get `Clone + PlainState` bounds (and `Refs` on the `Refs` impl).
//! The generated code names the traits as `::of_engine::…`; `of-engine` itself declares
//! `extern crate self as of_engine;` so the paths also resolve inside that crate.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Ts;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Field, Fields, Index, Type, parse_macro_input, parse_quote};

/// How a field takes part in `Refs::visit_refs`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefMode {
    Visit,
    Skip,
    GcOnly,
    ChainVersion,
}

fn ref_mode(attrs: &[Attribute]) -> syn::Result<RefMode> {
    let mut mode = RefMode::Visit;
    for a in attrs {
        if !a.path().is_ident("refs") {
            continue;
        }
        a.parse_nested_meta(|m| {
            mode = if m.path.is_ident("skip") {
                RefMode::Skip
            } else if m.path.is_ident("gc_only") {
                RefMode::GcOnly
            } else if m.path.is_ident("chain_version") {
                RefMode::ChainVersion
            } else {
                return Err(m.error("expected `skip`, `gc_only` or `chain_version`"));
            };
            Ok(())
        })?;
    }
    Ok(mode)
}

fn container_manual(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut manual = false;
    for a in attrs {
        if !a.path().is_ident("refs") {
            continue;
        }
        a.parse_nested_meta(|m| {
            if m.path.is_ident("manual") {
                manual = true;
                Ok(())
            } else {
                Err(m.error("expected `manual` on a type"))
            }
        })?;
    }
    Ok(manual)
}

/// The visit statement for one field, given an expression that is a reference to it.
fn visit_stmt(f: &Field, access: Ts) -> syn::Result<Ts> {
    let refs = quote!(::of_engine::canon::Refs);
    Ok(match ref_mode(&f.attrs)? {
        RefMode::Visit => quote! { #refs::visit_refs(#access, __v); },
        RefMode::Skip => quote! { let _ = #access; },
        RefMode::GcOnly => quote! { if !__v.canon_pass() { #refs::visit_refs(#access, __v); } },
        RefMode::ChainVersion => quote! { __v.visit_chain_version(*#access); },
    })
}

#[proc_macro_derive(StateClone, attributes(refs))]
pub fn derive_state_clone(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<Ts> {
    let name = &input.ident;
    let plain = quote!(::of_engine::plain::PlainState);
    let mut generics = input.generics.clone();
    for tp in generics.type_params_mut() {
        tp.bounds.push(parse_quote!(::core::clone::Clone));
        tp.bounds.push(parse_quote!(::of_engine::plain::PlainState));
    }
    let (impl_g, ty_g, where_c) = generics.split_for_impl();

    let mut field_types: Vec<&Type> = Vec::new();
    let manual_refs = container_manual(&input.attrs)?;
    let mut refs_body = quote! {};
    let (clone_body, clone_from_body) = match &input.data {
        Data::Struct(s) => {
            field_types.extend(s.fields.iter().map(|f| &f.ty));
            let mut stmts = Vec::new();
            for (i, f) in s.fields.iter().enumerate() {
                let access = match &f.ident {
                    Some(id) => quote!(&self.#id),
                    None => {
                        let ix = Index::from(i);
                        quote!(&self.#ix)
                    }
                };
                stmts.push(visit_stmt(f, access)?);
            }
            refs_body = quote! { #(#stmts)* };
            struct_bodies(&s.fields)
        }
        Data::Enum(e) => {
            if e.variants.is_empty() {
                return Err(syn::Error::new_spanned(name, "StateClone: empty enums are not supported"));
            }
            let mut clone_arms = Vec::new();
            let mut from_arms = Vec::new();
            let mut refs_arms = Vec::new();
            for v in &e.variants {
                let vn = &v.ident;
                match &v.fields {
                    Fields::Named(f) => {
                        let ids: Vec<_> = f.named.iter().map(|f| f.ident.clone().expect("named")).collect();
                        let mut st = Vec::new();
                        for (fi, id) in f.named.iter().zip(&ids) {
                            st.push(visit_stmt(fi, quote!(#id))?);
                        }
                        refs_arms.push(quote! { Self::#vn { #(#ids),* } => { #(#st)* } });
                    }
                    Fields::Unnamed(f) => {
                        let x: Vec<_> = (0..f.unnamed.len()).map(|i| format_ident!("__r{}", i)).collect();
                        let mut st = Vec::new();
                        for (fi, id) in f.unnamed.iter().zip(&x) {
                            st.push(visit_stmt(fi, quote!(#id))?);
                        }
                        refs_arms.push(quote! { Self::#vn(#(#x),*) => { #(#st)* } });
                    }
                    Fields::Unit => refs_arms.push(quote! { Self::#vn => {} }),
                }
                field_types.extend(v.fields.iter().map(|f| &f.ty));
                let vn = &v.ident;
                match &v.fields {
                    Fields::Named(f) => {
                        let ids: Vec<_> = f.named.iter().map(|f| f.ident.clone().expect("named")).collect();
                        let d: Vec<_> = ids.iter().map(|i| format_ident!("__d_{}", i)).collect();
                        let s: Vec<_> = ids.iter().map(|i| format_ident!("__s_{}", i)).collect();
                        clone_arms.push(quote! {
                            Self::#vn { #(#ids),* } => Self::#vn { #(#ids: ::core::clone::Clone::clone(#ids)),* }
                        });
                        from_arms.push(quote! {
                            (Self::#vn { #(#ids: #d),* }, Self::#vn { #(#ids: #s),* }) => {
                                #(::core::clone::Clone::clone_from(#d, #s);)*
                            }
                        });
                    }
                    Fields::Unnamed(f) => {
                        let n = f.unnamed.len();
                        let x: Vec<_> = (0..n).map(|i| format_ident!("__x{}", i)).collect();
                        let d: Vec<_> = (0..n).map(|i| format_ident!("__d{}", i)).collect();
                        let s: Vec<_> = (0..n).map(|i| format_ident!("__s{}", i)).collect();
                        clone_arms.push(quote! {
                            Self::#vn(#(#x),*) => Self::#vn(#(::core::clone::Clone::clone(#x)),*)
                        });
                        from_arms.push(quote! {
                            (Self::#vn(#(#d),*), Self::#vn(#(#s),*)) => {
                                #(::core::clone::Clone::clone_from(#d, #s);)*
                            }
                        });
                    }
                    Fields::Unit => {
                        clone_arms.push(quote! { Self::#vn => Self::#vn });
                        from_arms.push(quote! { (Self::#vn, Self::#vn) => {} });
                    }
                }
            }
            refs_body = quote! { match self { #(#refs_arms)* } };
            (
                quote! { match self { #(#clone_arms,)* } },
                quote! {
                    #[allow(unreachable_patterns)]
                    match (self, src) {
                        #(#from_arms)*
                        (__dst, __src) => *__dst = ::core::clone::Clone::clone(__src),
                    }
                },
            )
        }
        Data::Union(u) => {
            return Err(syn::Error::new_spanned(u.union_token, "StateClone: unions are not supported"));
        }
    };

    let refs_impl = if manual_refs {
        quote! {}
    } else {
        let mut rg = input.generics.clone();
        for tp in rg.type_params_mut() {
            tp.bounds.push(parse_quote!(::of_engine::canon::Refs));
        }
        let (rimpl_g, rty_g, rwhere_c) = rg.split_for_impl();
        quote! {
            #[automatically_derived]
            impl #rimpl_g ::of_engine::canon::Refs for #name #rty_g #rwhere_c {
                #[allow(unused_variables)]
                fn visit_refs(&self, __v: &mut dyn ::of_engine::canon::RefVisitor) {
                    #refs_body
                }
            }
        }
    };

    Ok(quote! {
        #refs_impl

        #[automatically_derived]
        impl #impl_g ::core::clone::Clone for #name #ty_g #where_c {
            #[inline]
            fn clone(&self) -> Self {
                #clone_body
            }
            #[inline]
            fn clone_from(&mut self, src: &Self) {
                #clone_from_body
            }
        }

        #[automatically_derived]
        impl #impl_g #plain for #name #ty_g #where_c {}

        const _: () = {
            fn __assert_plain<__T: ?::core::marker::Sized + #plain>() {}
            #[allow(dead_code)]
            fn __check #impl_g () #where_c {
                #(__assert_plain::<#field_types>();)*
            }
        };
    })
}

fn struct_bodies(fields: &Fields) -> (Ts, Ts) {
    match fields {
        Fields::Named(f) => {
            let ids: Vec<_> = f.named.iter().map(|f| f.ident.clone().expect("named")).collect();
            (
                quote! { Self { #(#ids: ::core::clone::Clone::clone(&self.#ids)),* } },
                quote! { #(::core::clone::Clone::clone_from(&mut self.#ids, &src.#ids);)* },
            )
        }
        Fields::Unnamed(f) => {
            let idx: Vec<Index> = (0..f.unnamed.len()).map(Index::from).collect();
            (
                quote! { Self(#(::core::clone::Clone::clone(&self.#idx)),*) },
                quote! { #(::core::clone::Clone::clone_from(&mut self.#idx, &src.#idx);)* },
            )
        }
        Fields::Unit => (quote! { Self }, quote! { let _ = src; }),
    }
}
