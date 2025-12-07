use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::token::Comma;
use syn::{
    Ident, ImplItem, Item, ItemImpl, LitStr, Meta, Path, Result as SynResult, parse_macro_input,
};

/// Helper object to parse comma-separated list of string literals.
struct LitStrList(Vec<LitStr>);

impl Parse for LitStrList {
    fn parse(input: ParseStream) -> SynResult<Self> {
        let literals = Punctuated::<LitStr, Comma>::parse_terminated(input)?;
        Ok(LitStrList(literals.into_iter().collect()))
    }
}

/// Helper object to parse comma-separated list of paths.
struct PathList(Vec<Path>);

impl Parse for PathList {
    fn parse(input: ParseStream) -> SynResult<Self> {
        let paths = Punctuated::<Path, Comma>::parse_terminated(input)?;
        Ok(PathList(paths.into_iter().collect()))
    }
}

/// Helper object to parse a string literal followed by a comma then a path.
struct LitStrPath {
    string_literal: LitStr,
    path: Path,
}

impl Parse for LitStrPath {
    fn parse(input: ParseStream) -> SynResult<Self> {
        let string_literal = input.parse::<LitStr>()?;
        input.parse::<Comma>()?;
        let path = input.parse::<Path>()?;
        Ok(LitStrPath {
            string_literal,
            path,
        })
    }
}

/// An attribute that applies to `impl` blocks.
/// Generates a sibling implementation for the [`Provider`] trait.
///
/// ```
/// #[instance("wasi:cli/exit@0.2.1", "wasi:cli/exit@0.2.2", "wasi:cli/exit@0.2.3")]
/// impl Exit {
///
///     #[function("exit")]
///     async fn exit(
///         context: wasmtime::StoreContextMut<'_, std::sync::Arc<crate::HostState>>,
///         parameters: (Result<(), ()>,),
///     ) -> anyhow::Result<()> {
///         // ...
///         Ok(())
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn instance(attribute: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the impl block.
    let impl_block = parse_macro_input!(item as ItemImpl);
    let self_name = &impl_block.self_ty;

    // Parse the attribute parameter as a comma-separated list of string literals.
    let instance_names = parse_macro_input!(attribute as LitStrList).0;

    // Extract functions with either a `#[function("name")]` attribute
    // or a `#[resource("name", Type)]` attribute.
    // In the latter case, the function implements the destructor for the resource.
    let mut functions: Vec<(LitStr, Ident)> = Vec::new();
    let mut resources: Vec<(LitStr, Path, Ident)> = Vec::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(function) = item {
            for attribute in &function.attrs {
                let path = attribute.path();
                if path.is_ident("function") {
                    if let Meta::List(meta_list) = &attribute.meta
                        && let Ok(export_name) = meta_list.parse_args::<LitStr>()
                    {
                        functions.push((export_name, function.sig.ident.clone()));
                    } else {
                        panic!(
                            "Expected 'function' attibute to have form #[function(\"export-name\")]",
                        );
                    }
                } else if path.is_ident("resource") {
                    if let Meta::List(meta_list) = &attribute.meta
                        && let Ok(parsed) = meta_list.parse_args::<LitStrPath>()
                    {
                        resources.push((
                            parsed.string_literal,
                            parsed.path,
                            function.sig.ident.clone(),
                        ));
                    } else {
                        panic!(
                            "Expected 'resource' attibute to have form #[resource(\"export-name\", Type)]",
                        );
                    }
                }
            }
        }
    }

    // Construct the body of the `provide` function.
    let provide_blocks = instance_names.into_iter().map(|instance_name| {
        let instance_blocks = functions
            .iter()
            .map(|(function_name, function_identifier)| {
                quote! {
                    instance.func_wrap_async(
                        #function_name,
                        |context, parameters| std::boxed::Box::new(Self::#function_identifier(context, parameters)),
                    )?;
                }
            }).chain(resources.iter().map(|(resource_name, resource_type, resource_destructor)| {
                quote! {
                    instance.resource_async(
                        #resource_name,
                        wasmtime::component::ResourceType::host::<#resource_type>(),
                        |context, index| std::boxed::Box::new(Self::#resource_destructor(context, index)),
                    )?;
                }
            }));
        quote! {
            {
                let mut instance = linker.instance(#instance_name)?;
                #(#instance_blocks)*
            }
        }
    });

    // Generate the output:
    // the original implementation block plus new implementation for `Provider`.
    let output = quote! {
        #impl_block

        impl Provider for #self_name {
            fn provide(
                &self,
                linker: &mut wasmtime::component::Linker<std::sync::Arc<crate::HostState>>,
            ) -> anyhow::Result<()> {
                #(#provide_blocks)*
                Ok(())
            }
        }
    };

    TokenStream::from(output)
}

/// This attribute is scanned in the processing of the [`instance`] attribute.
/// It does nothing on its own.
#[proc_macro_attribute]
pub fn function(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// This attribute is scanned in the processing of the [`instance`] attribute.
/// It does nothing on its own.
#[proc_macro_attribute]
pub fn resource(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// An attribute that applies to type declarations.
/// Used to combine several individual [providers](Provider)
/// into a single provider.
///
/// Generates an implementation for the `Provider` trait
/// that is a combination of the providers of each API member.
///
/// ```
/// #[instance("wasi:cli/exit@0.2.1", "wasi:cli/exit@0.2.2", "wasi:cli/exit@0.2.3")]
/// impl Exit {
///     // ...
/// }
///
/// #[api(Exit, Other, Another)]
/// struct Api;
/// ```
#[proc_macro_attribute]
pub fn api(attribute: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the type declaration.
    let type_declaration = parse_macro_input!(item as Item);
    let self_name = match &type_declaration {
        Item::Struct(structure) => &structure.ident,
        Item::Enum(enumeration) => &enumeration.ident,
        Item::Type(type_alias) => &type_alias.ident,
        _ => panic!("Expected 'api' attribute to be applied to a struct, enum, or type alias"),
    };

    // Parse the attribute parameter as a comma-separated list of type paths.
    let provider_types = parse_macro_input!(attribute as PathList).0;

    // Generate calls to each provider's `provide` method.
    let provide_calls = provider_types.iter().map(|provider_type| {
        quote! {
            #provider_type.provide(linker)?;
        }
    });

    // Generate the output:
    // the original type declaration plus new implementation for `Provider`.
    let output = quote! {
        #type_declaration

        impl Provider for #self_name {
            fn provide(
                &self,
                linker: &mut wasmtime::component::Linker<std::sync::Arc<crate::HostState>>,
            ) -> anyhow::Result<()> {
                #(#provide_calls)*
                Ok(())
            }
        }
    };

    TokenStream::from(output)
}
