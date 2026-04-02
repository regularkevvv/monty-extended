//! Proc macros that remove ABI boilerplate from Monty native extensions.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Attribute, Error, Expr, FnArg, GenericArgument, Ident, ImplItem, ImplItemFn, Item, ItemEnum, ItemImpl, ItemMod,
    LitStr, Meta, Pat, PatIdent, PathArguments, Result, Signature, Token, Type, Variant,
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
};

/// Generates typed handle wrappers and object-store helpers for an extension.
///
/// The annotated enum must contain only tuple variants with exactly one field.
/// Each variant becomes a typed handle wrapper plus `store_*`, `with_*`, and
/// `with_*_mut` helpers on the target extension type.
///
/// Example:
///
/// ```ignore
/// #[monty_handles(extension = MyExtension, module = "mymodule")]
/// enum StoredObject {
///     DataFrame(DataFrame),
///     Series(Series),
/// }
/// ```
#[proc_macro_attribute]
pub fn monty_handles(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as HandlesAttr);
    let item = parse_macro_input!(item as ItemEnum);
    match expand_monty_handles(attr, &item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Ergonomic alias for [`monty_handles`].
///
/// Identical semantics — generates the same typed handle wrappers and helpers —
/// but uses class-oriented vocabulary that mirrors PyO3's `#[pyclass]` pattern.
#[proc_macro_attribute]
pub fn monty_classes(attr: TokenStream, item: TokenStream) -> TokenStream {
    monty_handles(attr, item)
}

/// Generates the manifest, dispatch table, and C ABI entry point for an extension impl.
///
/// Methods inside the impl are exported by marking them with one of:
///
/// - `#[function()]` / `#[monty_function()]`
/// - `#[function(name = "PythonName")]` / `#[monty_function(name = "PythonName")]`
/// - `#[method()]` / `#[monty_method()]`
/// - `#[method(name = "python_name")]` / `#[monty_method(name = "python_name")]`
/// - `#[method(handle = "module.Type")]` / `#[monty_method(handle = "module.Type")]`
/// - `#[shutdown()]` / `#[monty_shutdown()]`
///
/// `#[method]` infers the exported handle type from the first non-injected
/// parameter when that parameter implements `MontyHandleType`, which is what
/// [`monty_handles`] generates.
#[proc_macro_attribute]
pub fn monty_extension(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as ExtensionAttr);
    let item = parse_macro_input!(item as ItemImpl);
    match expand_monty_extension(attr, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Module-oriented macro for defining Monty extensions.
///
/// Supports two usage patterns:
///
/// **Impl-level** (backward compatible): applied to an `impl` block, behaves
/// identically to [`monty_extension`]. Inner methods are tagged with
/// `#[monty_function]`, `#[monty_method]`, `#[monty_shutdown]`.
///
/// **Module-level** (new): applied to a `mod` block, enables a declarative
/// PyO3-inspired API where `#[monty_class]` structs, `#[monty_function]` free
/// functions, and `#[monty_methods] impl ClassName` blocks replace the manual
/// enum + struct + single-impl pattern. The macro auto-generates `Extension`,
/// `StoredObject`, typed handles, and all dispatch/ABI glue.
#[proc_macro_attribute]
pub fn monty_module(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Try the new mod-level expansion first
    if let Ok(module) = syn::parse::<ItemMod>(item.clone()) {
        let parsed_attr = match syn::parse::<ExtensionAttr>(attr) {
            Ok(a) => a,
            Err(e) => return e.to_compile_error().into(),
        };
        return match expand_monty_module_mod(parsed_attr, module) {
            Ok(tokens) => tokens.into(),
            Err(e) => e.to_compile_error().into(),
        };
    }
    // Fall back to impl-level expansion (backward compatible)
    monty_extension(attr, item)
}

/// Marks a struct as a class managed by a `#[monty_module]` module.
///
/// When used inside a `#[monty_module] mod`, this marker triggers generation of
/// a typed handle wrapper, a `StoredObject` enum variant, and store/with/with_mut
/// helpers on the auto-generated `Extension` struct.
///
/// Standalone usage (outside `#[monty_module]`) is a no-op pass-through.
#[proc_macro_attribute]
pub fn monty_class(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return Error::new(Span::call_site(), "#[monty_class] does not accept arguments")
            .to_compile_error()
            .into();
    }
    item
}

/// Marks an `impl ClassName` block as containing methods for a `#[monty_class]`.
///
/// When used inside a `#[monty_module] mod`, each function in the block is
/// extracted as a free function and registered for handle-method dispatch grouped
/// by the class type name. Individual methods may use `#[monty_method(name = "...")]`
/// to override the exported Python name.
///
/// Standalone usage (outside `#[monty_module]`) is a no-op pass-through.
#[proc_macro_attribute]
pub fn monty_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return Error::new(Span::call_site(), "#[monty_methods] does not accept arguments")
            .to_compile_error()
            .into();
    }
    item
}

/// Parsed arguments for [`monty_handles`].
struct HandlesAttr {
    /// The extension type that owns the `objects` and `next_id` fields.
    extension: syn::Path,
    /// The module name that will own the generated handles.
    module: LitStr,
}

impl Parse for HandlesAttr {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut extension = None;
        let mut module = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "extension" => extension = Some(input.parse()?),
                "module" => module = Some(input.parse()?),
                _ => return Err(Error::new(key.span(), "unsupported monty_handles argument")),
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(Self {
            extension: extension
                .ok_or_else(|| Error::new(Span::call_site(), "missing `extension = TypeName` argument"))?,
            module: module.ok_or_else(|| Error::new(Span::call_site(), "missing `module = \"name\"` argument"))?,
        })
    }
}

/// Parsed arguments for [`monty_extension`].
struct ExtensionAttr {
    /// The exported Python module name.
    name: LitStr,
    /// The semantic version string.
    version: LitStr,
    /// The skill expression embedded in the manifest.
    skill: Expr,
    /// Optional type stub expression embedded in the manifest.
    stubs: Option<Expr>,
}

impl Parse for ExtensionAttr {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut skill = None;
        let mut stubs = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => name = Some(input.parse()?),
                "version" => version = Some(input.parse()?),
                "skill" => skill = Some(input.parse()?),
                "stubs" => stubs = Some(input.parse()?),
                _ => return Err(Error::new(key.span(), "unsupported monty_extension argument")),
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(Self {
            name: name.ok_or_else(|| Error::new(Span::call_site(), "missing `name = \"module\"` argument"))?,
            version: version.ok_or_else(|| Error::new(Span::call_site(), "missing `version = \"x.y.z\"` argument"))?,
            skill: skill.ok_or_else(|| Error::new(Span::call_site(), "missing `skill = EXPR` argument"))?,
            stubs,
        })
    }
}

/// Authoring role assigned to an exported impl method.
enum ExportKind {
    /// A top-level module function.
    Function(FunctionAttr),
    /// A method exported on a handle type.
    Method(MethodAttr),
    /// The shutdown hook for the extension.
    Shutdown,
}

/// Parsed arguments for `#[function(...)]`.
#[derive(Default)]
struct FunctionAttr {
    /// Optional Python-visible name override.
    name: Option<LitStr>,
}

/// Parsed arguments for `#[method(...)]`.
#[derive(Default)]
struct MethodAttr {
    /// Optional Python-visible name override.
    name: Option<LitStr>,
    /// Optional explicit handle type when the first parameter is not a typed handle.
    handle: Option<LitStr>,
}

/// A parsed exported function or method.
struct ExportedMethod {
    /// The Python-visible export name.
    export_name: LitStr,
    /// The method kind.
    kind: ExportedMethodKind,
    /// The generated dispatch body for invoking the Rust method.
    dispatch_body: TokenStream2,
}

/// Function or method metadata used during code generation.
enum ExportedMethodKind {
    /// Top-level function metadata.
    Function,
    /// Handle method metadata.
    Method {
        /// The exported handle type string.
        handle_type: TokenStream2,
        /// Human-readable short type name for `AttributeError`.
        short_type_name: TokenStream2,
    },
}

/// Expands [`monty_handles`] into the original enum plus generated helpers.
fn expand_monty_handles(attr: HandlesAttr, item: &ItemEnum) -> Result<TokenStream2> {
    let HandlesAttr { extension, module } = attr;
    let enum_ident = &item.ident;
    let module_name = module.value();

    let generated = item
        .variants
        .iter()
        .map(|variant| expand_handle_variant(enum_ident, variant, &extension, &module_name))
        .collect::<Result<Vec<_>>>()?;

    Ok(quote! {
        #item
        #(#generated)*
    })
}

/// Expands a single stored-object variant into a typed handle and helper methods.
fn expand_handle_variant(
    enum_ident: &Ident,
    variant: &Variant,
    extension: &syn::Path,
    module_name: &str,
) -> Result<TokenStream2> {
    let variant_ident = &variant.ident;
    let variant_type = extract_variant_type(variant)?;
    let handle_ident = format_ident!("{variant_ident}Handle");
    let type_name = LitStr::new(&format!("{module_name}.{variant_ident}"), variant_ident.span());
    let expected_handle_text = LitStr::new(&format!("a {module_name}.{variant_ident} handle"), variant_ident.span());
    let short_type_name = LitStr::new(&variant_ident.to_string(), variant_ident.span());
    let snake_name = snake_case(&variant_ident.to_string());
    let store_ident = format_ident!("store_{snake_name}");
    let with_ident = format_ident!("with_{snake_name}");
    let with_mut_ident = format_ident!("with_{snake_name}_mut");

    Ok(quote! {
        #[doc = "Typed wrapper around an `ExtHandle` for this stored object variant."]
        #[derive(Clone, Debug)]
        pub struct #handle_ident(pub ::monty_extension_api::ExtHandle);

        impl #handle_ident {
            #[doc = "Returns the borrowed ABI handle backing this typed handle."]
            #[must_use]
            pub fn as_ext_handle(&self) -> &::monty_extension_api::ExtHandle {
                &self.0
            }

            #[doc = "Returns the extension-local handle ID."]
            #[must_use]
            pub fn handle_id(&self) -> u64 {
                self.0.handle_id
            }
        }

        impl ::monty_extension_api::TryIntoExtValue for #handle_ident {
            fn try_into_ext_value(self) -> ::monty_extension_api::ExtValueResult<::monty_extension_api::ExtValue> {
                Ok(::monty_extension_api::ExtValue::Handle(self.0))
            }
        }

        impl<'a> ::monty_extension_api::FromExtValue<'a> for #handle_ident {
            fn from_ext_value(
                value: &'a ::monty_extension_api::ExtValue,
                function_name: &str,
                parameter_name: &str,
            ) -> ::monty_extension_api::ExtValueResult<Self> {
                let handle = <::monty_extension_api::ExtHandle as ::monty_extension_api::FromExtValue>::from_ext_value(
                    value,
                    function_name,
                    parameter_name,
                )?;
                if handle.type_name.as_str() != #type_name {
                    return Err(::monty_extension_api::ExtError::argument_type(
                        function_name,
                        parameter_name,
                        #expected_handle_text,
                    ));
                }
                Ok(Self(handle))
            }
        }

        impl ::monty_extension_api::MontyHandleType for #handle_ident {
            const TYPE_NAME: &'static str = #type_name;

            fn as_ext_handle(&self) -> &::monty_extension_api::ExtHandle {
                &self.0
            }

            fn into_ext_handle(self) -> ::monty_extension_api::ExtHandle {
                self.0
            }
        }

        impl #extension {
            #[doc = "Stores a new object in the extension store and returns a typed handle."]
            pub fn #store_ident(&self, value: #variant_type) -> #handle_ident {
                let mut next = self.next_id.lock().unwrap_or_else(|error| error.into_inner());
                let id = *next;
                *next += 1;
                self.objects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(id, #enum_ident::#variant_ident(value));
                #handle_ident(::monty_extension_api::ExtHandle {
                    type_name: #type_name.into(),
                    handle_id: id,
                    extension_id: #module_name.into(),
                })
            }

            #[doc = "Borrows an object of this variant for the duration of the callback."]
            pub fn #with_ident<T>(
                &self,
                handle: &#handle_ident,
                function_name: &str,
                callback: impl FnOnce(&#variant_type) -> ::monty_extension_api::ExtValueResult<T>,
            ) -> ::monty_extension_api::ExtValueResult<T> {
                let objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
                let object = objects.get(&handle.handle_id()).ok_or_else(|| {
                    ::monty_extension_api::ExtError::invalid_handle(function_name, #short_type_name, handle.handle_id())
                })?;
                match object {
                    #enum_ident::#variant_ident(value) => callback(value),
                    _ => Err(::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )),
                }
            }

            #[doc = "Mutably borrows an object of this variant for the duration of the callback."]
            pub fn #with_mut_ident<T>(
                &self,
                handle: &#handle_ident,
                function_name: &str,
                callback: impl FnOnce(&mut #variant_type) -> ::monty_extension_api::ExtValueResult<T>,
            ) -> ::monty_extension_api::ExtValueResult<T> {
                let mut objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
                let object = objects.get_mut(&handle.handle_id()).ok_or_else(|| {
                    ::monty_extension_api::ExtError::invalid_handle(function_name, #short_type_name, handle.handle_id())
                })?;
                match object {
                    #enum_ident::#variant_ident(value) => callback(value),
                    _ => Err(::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )),
                }
            }
        }
    })
}

/// Expands [`monty_extension`] into the original impl plus generated trait glue.
fn expand_monty_extension(attr: ExtensionAttr, mut item: ItemImpl) -> Result<TokenStream2> {
    validate_extension_impl(&item)?;

    let self_ty = (*item.self_ty).clone();
    let mut functions = Vec::new();
    let mut methods = Vec::new();
    let mut shutdown = None;

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let export = extract_export_kind(&mut method.attrs)?;
        match export {
            Some(ExportKind::Function(function_attr)) => {
                functions.push(build_function_export(method, function_attr)?);
            }
            Some(ExportKind::Method(method_attr)) => {
                methods.push(build_method_export(method, method_attr)?);
            }
            Some(ExportKind::Shutdown) => {
                if shutdown.is_some() {
                    return Err(Error::new(method.span(), "only one #[shutdown] method is allowed"));
                }
                shutdown = Some(method.sig.ident.clone());
            }
            None => {}
        }
    }

    let function_manifest_entries = functions.iter().map(|function| {
        let export_name = &function.export_name;
        quote! {
            ::monty_extension_api::ExtFunctionDecl {
                name: #export_name.into(),
                is_native: true,
            }
        }
    });

    let function_dispatch_arms = functions.iter().map(generate_function_dispatch_arm);
    let method_types = collect_method_types(&methods);
    let method_type_arms = method_types.iter().map(|method_type| {
        let handle_type = &method_type.handle_type;
        let short_type_name = &method_type.short_type_name;
        let arms = method_type
            .methods
            .iter()
            .map(|method| generate_method_dispatch_arm(method));
        quote! {
            __handle_type if __handle_type == #handle_type => match method.as_str() {
                #(#arms,)*
                _ => {
                    return ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(
                        ::monty_extension_api::ExtError::attribute_error(#short_type_name, method.as_str())
                    ));
                }
            }
        }
    });

    let ExtensionAttr {
        name,
        version,
        skill,
        stubs,
    } = attr;

    let type_stub_expr = if let Some(stubs) = stubs {
        quote! {
            ::monty_extension_api::__private::ROption::RSome((#stubs).into())
        }
    } else {
        quote! { ::monty_extension_api::__private::ROption::RNone }
    };

    let shutdown_call = match shutdown {
        Some(shutdown_method) => quote! {
            <#self_ty>::#shutdown_method(self);
        },
        None => quote! {},
    };

    Ok(quote! {
        #item

        impl ::monty_extension_api::MontyExtension for #self_ty {
            fn manifest(&self) -> ::monty_extension_api::ExtManifest {
                ::monty_extension_api::ExtManifest {
                    module_name: #name.into(),
                    functions: ::monty_extension_api::__private::RVec::from(vec![#(#function_manifest_entries),*]),
                    type_stub_source: #type_stub_expr,
                    skill: (#skill).into(),
                    version: #version.into(),
                }
            }

            fn call(
                &self,
                function_name: ::monty_extension_api::__private::RStr<'_>,
                args: ::monty_extension_api::ExtArgs,
                ctx: &::monty_extension_api::ExtContext,
            ) -> ::monty_extension_api::ExtResult {
                match function_name.as_str() {
                    #(#function_dispatch_arms,)*
                    _ => ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(
                        ::monty_extension_api::ExtError::unknown_function(#name, function_name.as_str())
                    )),
                }
            }

            fn call_method(
                &self,
                handle: &::monty_extension_api::ExtHandle,
                method: ::monty_extension_api::__private::RStr<'_>,
                args: ::monty_extension_api::ExtArgs,
                ctx: &::monty_extension_api::ExtContext,
            ) -> ::monty_extension_api::ExtResult {
                match handle.type_name.as_str() {
                    #(#method_type_arms,)*
                    _ => {
                        let __short_type = handle
                            .type_name
                            .as_str()
                            .rsplit('.')
                            .next()
                            .unwrap_or(handle.type_name.as_str());
                        ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(
                            ::monty_extension_api::ExtError::attribute_error(__short_type, method.as_str())
                        ))
                    }
                }
            }

            fn shutdown(&self) {
                #shutdown_call
            }
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn monty_extension_entry() -> ::monty_extension_api::ExtensionEntry {
            ::monty_extension_api::ExtensionEntry {
                api_version: ::monty_extension_api::API_VERSION,
                create: __monty_create_extension,
            }
        }

        extern "C" fn __monty_create_extension(
        ) -> ::monty_extension_api::MontyExtension_TO<'static, ::monty_extension_api::__private::RBox<()>> {
            ::monty_extension_api::MontyExtension_TO::from_value(
                <#self_ty>::new(),
                ::monty_extension_api::__private::TD_Opaque,
            )
        }
    })
}

/// Ensures the macro is attached to a plain inherent impl block.
fn validate_extension_impl(item: &ItemImpl) -> Result<()> {
    if item.trait_.is_some() {
        return Err(Error::new(
            item.span(),
            "#[monty_extension] only supports inherent impl blocks",
        ));
    }
    Ok(())
}

/// Extracts a single-field tuple variant type.
fn extract_variant_type(variant: &Variant) -> Result<Type> {
    let syn::Fields::Unnamed(fields) = &variant.fields else {
        return Err(Error::new(
            variant.span(),
            "#[monty_handles] variants must be tuple variants with exactly one field",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(Error::new(
            variant.span(),
            "#[monty_handles] variants must contain exactly one field",
        ));
    }
    Ok(fields.unnamed.first().expect("len checked").ty.clone())
}

/// Removes and parses a supported helper attribute from a method.
///
/// Recognises both the classic names (`function`, `method`, `shutdown`) and the
/// ergonomic aliases (`monty_function`, `monty_method`, `monty_shutdown`) so
/// extensions can be authored with either vocabulary.
fn extract_export_kind(attrs: &mut Vec<Attribute>) -> Result<Option<ExportKind>> {
    let mut found = None;
    let mut retained = Vec::with_capacity(attrs.len());

    for attr in attrs.drain(..) {
        if is_function_attr(&attr) {
            if found.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "multiple export attributes on the same method are not supported",
                ));
            }
            found = Some(ExportKind::Function(parse_function_attr(&attr)?));
        } else if is_method_attr(&attr) {
            if found.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "multiple export attributes on the same method are not supported",
                ));
            }
            found = Some(ExportKind::Method(parse_method_attr(&attr)?));
        } else if is_shutdown_attr(&attr) {
            if found.is_some() {
                return Err(Error::new(
                    attr.span(),
                    "multiple export attributes on the same method are not supported",
                ));
            }
            let valid_shutdown =
                matches!(&attr.meta, Meta::Path(_)) || matches!(&attr.meta, Meta::List(list) if list.tokens.is_empty());
            if !valid_shutdown {
                return Err(Error::new(attr.span(), "#[shutdown] does not accept arguments"));
            }
            found = Some(ExportKind::Shutdown);
        } else {
            retained.push(attr);
        }
    }

    *attrs = retained;
    Ok(found)
}

/// Returns `true` when `attr` is `#[function(...)]` or `#[monty_function(...)]`.
fn is_function_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("function") || attr.path().is_ident("monty_function")
}

/// Returns `true` when `attr` is `#[method(...)]` or `#[monty_method(...)]`.
fn is_method_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("method") || attr.path().is_ident("monty_method")
}

/// Returns `true` when `attr` is `#[shutdown(...)]` or `#[monty_shutdown(...)]`.
fn is_shutdown_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("shutdown") || attr.path().is_ident("monty_shutdown")
}

/// Parses `#[function(...)]`.
fn parse_function_attr(attr: &Attribute) -> Result<FunctionAttr> {
    let mut output = FunctionAttr::default();
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            output.name = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("unsupported #[function(...)] argument"))
        }
    })?;
    Ok(output)
}

/// Parses `#[method(...)]`.
fn parse_method_attr(attr: &Attribute) -> Result<MethodAttr> {
    let mut output = MethodAttr::default();
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            output.name = Some(meta.value()?.parse()?);
            Ok(())
        } else if meta.path.is_ident("handle") {
            output.handle = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("unsupported #[method(...)] argument"))
        }
    })?;
    Ok(output)
}

/// Builds metadata for an exported top-level function.
fn build_function_export(method: &ImplItemFn, attr: FunctionAttr) -> Result<ExportedMethod> {
    validate_method_signature(method)?;
    let export_name = attr
        .name
        .unwrap_or_else(|| LitStr::new(&method.sig.ident.to_string(), method.sig.ident.span()));
    let dispatch_body = build_dispatch_body(method, &export_name, CallMode::Function)?;

    Ok(ExportedMethod {
        export_name,
        kind: ExportedMethodKind::Function,
        dispatch_body,
    })
}

/// Builds metadata for an exported handle method.
fn build_method_export(method: &ImplItemFn, attr: MethodAttr) -> Result<ExportedMethod> {
    validate_method_signature(method)?;
    let export_name = attr
        .name
        .unwrap_or_else(|| LitStr::new(&method.sig.ident.to_string(), method.sig.ident.span()));
    let handle_argument = find_handle_argument(method)?;
    if attr.handle.is_none() && !can_infer_handle_type(handle_argument.argument_type) {
        return Err(Error::new(
            handle_argument.argument_type.span(),
            "#[method] needs `handle = \"module.Type\"` when the handle parameter is `ExtHandle`, `&ExtHandle`, or a reference type",
        ));
    }
    let explicit_handle = attr.handle;
    let handle_type = if let Some(ref handle) = explicit_handle {
        quote! { #handle }
    } else {
        let ty = handle_argument.argument_type;
        quote! { <#ty as ::monty_extension_api::MontyHandleType>::TYPE_NAME }
    };
    let short_type_name = if let Some(ref handle) = explicit_handle {
        let short = LitStr::new(&short_type_name(&handle.value()), handle.span());
        quote! { #short }
    } else {
        let ty = handle_argument.argument_type;
        quote! {
            <#ty as ::monty_extension_api::MontyHandleType>::TYPE_NAME
                .rsplit('.')
                .next()
                .unwrap_or(<#ty as ::monty_extension_api::MontyHandleType>::TYPE_NAME)
        }
    };
    let dispatch_body = build_dispatch_body(method, &export_name, CallMode::Method)?;

    Ok(ExportedMethod {
        export_name,
        kind: ExportedMethodKind::Method {
            handle_type,
            short_type_name,
        },
        dispatch_body,
    })
}

/// Common validation shared by exported functions and methods.
fn validate_method_signature(method: &ImplItemFn) -> Result<()> {
    if method.sig.asyncness.is_some() {
        return Err(Error::new(
            method.sig.asyncness.span(),
            "async extension exports are not supported",
        ));
    }
    if method.sig.generics.params.iter().next().is_some() {
        return Err(Error::new(
            method.sig.generics.span(),
            "generic extension exports are not supported",
        ));
    }

    let Some(receiver) = method.sig.receiver() else {
        return Err(Error::new(
            method.sig.span(),
            "exported extension methods must take &self",
        ));
    };
    if receiver.reference.is_none() || receiver.mutability.is_some() {
        return Err(Error::new(
            receiver.span(),
            "exported extension methods must take &self",
        ));
    }

    Ok(())
}

/// Generates the full dispatch body needed to invoke an exported method body.
fn build_dispatch_body(method: &ImplItemFn, export_name: &LitStr, mode: CallMode) -> Result<TokenStream2> {
    let mut setup = Vec::new();
    let mut invoke_arguments = Vec::new();
    let mut arg_index = 0usize;
    let mut handle_consumed = false;

    for input in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(argument) = input else {
            return Err(Error::new(input.span(), "unexpected receiver form"));
        };
        let ident = extract_argument_ident(&argument.pat)?;
        let ty = &argument.ty;

        if is_ext_context(ty) {
            invoke_arguments.push(quote! { ctx });
            continue;
        }
        if is_ext_args(ty) {
            invoke_arguments.push(quote! { &args });
            continue;
        }

        if matches!(mode, CallMode::Method) && !handle_consumed {
            handle_consumed = true;
            if is_ref_ext_handle(ty) {
                invoke_arguments.push(quote! { handle });
            } else if is_ext_handle(ty) {
                invoke_arguments.push(quote! { handle.clone() });
            } else {
                setup.push(quote! {
                    let __handle_value = ::monty_extension_api::ExtValue::Handle(handle.clone());
                    let #ident: #ty = match <#ty as ::monty_extension_api::FromExtValue>::from_ext_value(
                        &__handle_value,
                        #export_name,
                        stringify!(#ident),
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(error));
                        }
                    };
                });
                invoke_arguments.push(quote! { #ident });
            }
            continue;
        }

        if let Some(inner) = option_inner_type(ty) {
            setup.push(quote! {
                let #ident: Option<#inner> = match args.optional_argument::<#inner>(#arg_index, #export_name, stringify!(#ident)) {
                    Ok(value) => value,
                    Err(error) => {
                        return ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(error));
                    }
                };
            });
            invoke_arguments.push(quote! { #ident });
        } else {
            setup.push(quote! {
                let #ident: #ty = match args.argument::<#ty>(#arg_index, #export_name, stringify!(#ident)) {
                    Ok(value) => value,
                    Err(error) => {
                        return ::monty_extension_api::into_ext_result(Err::<::monty_extension_api::ExtValue, _>(error));
                    }
                };
            });
            invoke_arguments.push(quote! { #ident });
        }
        arg_index += 1;
    }

    if matches!(mode, CallMode::Method) && !handle_consumed {
        return Err(Error::new(
            method.sig.span(),
            "#[method] exports must take a handle parameter after &self",
        ));
    }

    let rust_name = &method.sig.ident;
    Ok(quote! {
        {
            #(#setup)*
            ::monty_extension_api::into_ext_result(<Self>::#rust_name(self, #(#invoke_arguments),*))
        }
    })
}

/// Finds the first non-injected parameter in a method export, which is the handle.
fn find_handle_argument(method: &ImplItemFn) -> Result<HandleArgument<'_>> {
    for input in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(argument) = input else {
            continue;
        };
        if is_ext_context(&argument.ty) || is_ext_args(&argument.ty) {
            continue;
        }
        return Ok(HandleArgument {
            argument_type: &argument.ty,
        });
    }

    Err(Error::new(
        method.sig.span(),
        "#[method] exports must take a handle parameter after &self",
    ))
}

/// Generates one top-level function dispatch arm.
fn generate_function_dispatch_arm(function: &ExportedMethod) -> TokenStream2 {
    let export_name = &function.export_name;
    let dispatch_body = &function.dispatch_body;

    quote! {
        #export_name => #dispatch_body
    }
}

/// Generates one handle method dispatch arm.
fn generate_method_dispatch_arm(method: &ExportedMethod) -> TokenStream2 {
    let export_name = &method.export_name;
    let dispatch_body = &method.dispatch_body;

    quote! {
        #export_name => #dispatch_body
    }
}

/// Groups exported methods by their handle type string.
fn collect_method_types<'a>(methods: &'a [ExportedMethod]) -> Vec<MethodType<'a>> {
    let mut grouped: Vec<MethodType<'a>> = Vec::new();

    for method in methods {
        let ExportedMethodKind::Method {
            handle_type,
            short_type_name,
        } = &method.kind
        else {
            continue;
        };

        if let Some(existing) = grouped
            .iter_mut()
            .find(|existing| existing.handle_type.to_string() == handle_type.to_string())
        {
            existing.methods.push(method);
            continue;
        }

        grouped.push(MethodType {
            handle_type: handle_type.clone(),
            short_type_name: short_type_name.clone(),
            methods: vec![method],
        });
    }

    grouped
}

/// Extracts a plain identifier from an argument pattern.
fn extract_argument_ident(pattern: &Pat) -> Result<Ident> {
    let Pat::Ident(PatIdent { ident, .. }) = pattern else {
        return Err(Error::new(
            pattern.span(),
            "exported arguments must use simple identifiers",
        ));
    };
    Ok(ident.clone())
}

/// Returns whether a type is `ExtContext` or `&ExtContext`.
fn is_ext_context(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => type_path_ends_with(&reference.elem, "ExtContext"),
        _ => type_path_ends_with(ty, "ExtContext"),
    }
}

/// Returns whether a type is `ExtArgs` or `&ExtArgs`.
fn is_ext_args(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => type_path_ends_with(&reference.elem, "ExtArgs"),
        _ => type_path_ends_with(ty, "ExtArgs"),
    }
}

/// Returns whether a type is exactly `ExtHandle`.
fn is_ext_handle(ty: &Type) -> bool {
    type_path_ends_with(ty, "ExtHandle")
}

/// Returns whether a type is `&ExtHandle`.
fn is_ref_ext_handle(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => type_path_ends_with(&reference.elem, "ExtHandle"),
        _ => false,
    }
}

/// Returns the inner type when `ty` is `Option<T>`.
fn option_inner_type(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let last = type_path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = arguments.args.first()? else {
        return None;
    };
    Some(inner.clone())
}

/// Returns whether a method handle parameter can infer `MontyHandleType`.
fn can_infer_handle_type(ty: &Type) -> bool {
    !matches!(ty, Type::Reference(_)) && !is_ext_handle(ty)
}

/// Returns whether the last path segment of `ty` matches `ident`.
fn type_path_ends_with(ty: &Type, ident: &str) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == ident)
}

/// Converts a CamelCase identifier into snake_case for generated helper names.
fn snake_case(input: &str) -> String {
    let mut output = String::with_capacity(input.len() + 4);
    for (index, ch) in input.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                output.push('_');
            }
            output.extend(ch.to_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

/// Returns the final component of a dotted Python type name.
fn short_type_name(type_name: &str) -> String {
    type_name.rsplit('.').next().unwrap_or(type_name).to_string()
}

/// Call mode used when generating argument extraction.
#[derive(Clone, Copy)]
enum CallMode {
    /// A top-level function call dispatched through `call`.
    Function,
    /// A handle method call dispatched through `call_method`.
    Method,
}

/// Borrowed metadata for a method handle parameter.
struct HandleArgument<'a> {
    /// The declared Rust type of the handle parameter.
    argument_type: &'a Type,
}

/// Group of methods that share the same exported handle type.
struct MethodType<'a> {
    /// The exported handle type string.
    handle_type: TokenStream2,
    /// The short type name used in `AttributeError`.
    short_type_name: TokenStream2,
    /// All methods exported for this handle type.
    methods: Vec<&'a ExportedMethod>,
}

// ─── Module-level macro expansion ────────────────────────────────────────────

/// A class definition collected from a `#[monty_class]` struct inside a
/// `#[monty_module] mod` block.
struct ModClassDef {
    /// The struct identifier (e.g., `DataFrame`).
    ident: Ident,
}

/// A top-level function export collected from a `#[monty_function]` free
/// function inside a `#[monty_module] mod` block.
struct ModFunctionDef {
    /// The Python-visible export name.
    export_name: LitStr,
    /// The generated dispatch body that extracts arguments and calls the function.
    dispatch_body: TokenStream2,
}

/// A method export collected from a function inside a `#[monty_methods] impl`
/// block within a `#[monty_module] mod`.
struct ModMethodDef {
    /// The class this method belongs to (e.g., `DataFrame`).
    class_ident: Ident,
    /// The Python-visible export name.
    export_name: LitStr,
    /// The generated dispatch body.
    dispatch_body: TokenStream2,
}

/// Methods grouped by their class type name for dispatch code generation.
struct ModMethodGroup<'a> {
    /// Fully-qualified type name (e.g., `"datatools.DataFrame"`).
    type_name: LitStr,
    /// Short type name for error messages (e.g., `"DataFrame"`).
    short_name: LitStr,
    /// All methods exported for this class.
    methods: Vec<&'a ModMethodDef>,
}

/// Expands `#[monty_module(...)]` applied to a `mod` block.
///
/// Parses the module contents, collects class definitions, function exports,
/// method exports, and a shutdown hook, then generates all boilerplate:
/// `StoredObject` enum, `Extension` struct, typed handle wrappers with
/// store/with helpers, `MontyExtension` trait impl, and C ABI entry point.
fn expand_monty_module_mod(attr: ExtensionAttr, module: ItemMod) -> Result<TokenStream2> {
    let module_vis = &module.vis;
    let module_ident = &module.ident;
    let module_attrs = &module.attrs;

    let Some((_brace, items)) = module.content else {
        return Err(Error::new(
            module_ident.span(),
            "#[monty_module] requires an inline module body (`mod name { ... }` not `mod name;`)",
        ));
    };

    let module_name = &attr.name;
    let module_name_str = module_name.value();

    let mut class_defs: Vec<ModClassDef> = Vec::new();
    let mut function_defs: Vec<ModFunctionDef> = Vec::new();
    let mut method_defs: Vec<ModMethodDef> = Vec::new();
    let mut shutdown_ident: Option<Ident> = None;
    let mut output_items: Vec<TokenStream2> = Vec::new();

    for item in items {
        match item {
            Item::Struct(mut struct_item) => {
                if strip_named_attr(&mut struct_item.attrs, "monty_class") {
                    class_defs.push(ModClassDef {
                        ident: struct_item.ident.clone(),
                    });
                }
                output_items.push(quote! { #struct_item });
            }
            Item::Fn(mut fn_item) => {
                if let Some(func_attr) = take_function_export_attr(&mut fn_item.attrs)? {
                    validate_mod_function_sig(&fn_item.sig)?;
                    let export_name = func_attr
                        .name
                        .unwrap_or_else(|| LitStr::new(&fn_item.sig.ident.to_string(), fn_item.sig.ident.span()));
                    let dispatch_body = build_mod_dispatch_body(&fn_item.sig, &export_name, CallMode::Function)?;
                    function_defs.push(ModFunctionDef {
                        export_name,
                        dispatch_body,
                    });
                    output_items.push(quote! { #fn_item });
                } else if take_shutdown_export_attr(&mut fn_item.attrs)? {
                    if shutdown_ident.is_some() {
                        return Err(Error::new(
                            fn_item.sig.ident.span(),
                            "only one #[monty_shutdown] function is allowed",
                        ));
                    }
                    shutdown_ident = Some(fn_item.sig.ident.clone());
                    output_items.push(quote! { #fn_item });
                } else {
                    output_items.push(quote! { #fn_item });
                }
            }
            Item::Impl(mut impl_item) => {
                if strip_named_attr(&mut impl_item.attrs, "monty_methods") {
                    let class_ident = extract_impl_self_ident(&impl_item)?;

                    for impl_member in &mut impl_item.items {
                        let ImplItem::Fn(method) = impl_member else {
                            continue;
                        };
                        let name_override = take_method_name_override(&mut method.attrs)?;
                        let export_name = name_override
                            .unwrap_or_else(|| LitStr::new(&method.sig.ident.to_string(), method.sig.ident.span()));
                        validate_mod_function_sig(&method.sig)?;
                        let dispatch_body = build_mod_dispatch_body(&method.sig, &export_name, CallMode::Method)?;
                        method_defs.push(ModMethodDef {
                            class_ident: class_ident.clone(),
                            export_name,
                            dispatch_body,
                        });
                    }

                    for impl_member in impl_item.items {
                        if let ImplItem::Fn(method) = impl_member {
                            let method_attrs = &method.attrs;
                            let method_vis = &method.vis;
                            let method_sig = &method.sig;
                            let method_block = &method.block;
                            output_items.push(quote! {
                                #(#method_attrs)*
                                #method_vis #method_sig #method_block
                            });
                        }
                    }
                } else {
                    output_items.push(quote! { #impl_item });
                }
            }
            other => {
                output_items.push(quote! { #other });
            }
        }
    }

    let enum_variants: Vec<_> = class_defs
        .iter()
        .map(|c| {
            let ident = &c.ident;
            quote! { #ident(#ident) }
        })
        .collect();

    let stored_object_enum = quote! {
        /// Enum holding all class instances managed by this extension's object store.
        #[doc(hidden)]
        #[expect(private_interfaces, reason = "class structs are intentionally module-private")]
        pub(crate) enum StoredObject {
            #(#enum_variants),*
        }
    };

    let extension_struct = quote! {
        /// Auto-generated extension state holding the object store and handle counter.
        ///
        /// Extension functions receive this as their first parameter (`ext: &Extension`).
        pub struct Extension {
            /// All live stored objects keyed by handle ID.
            pub(crate) objects: ::std::sync::Mutex<::std::collections::HashMap<u64, StoredObject>>,
            /// Monotonically increasing handle ID counter.
            pub(crate) next_id: ::std::sync::Mutex<u64>,
        }

        impl Extension {
            /// Creates a new extension instance with an empty object store.
            pub fn new() -> Self {
                Self {
                    objects: ::std::sync::Mutex::new(::std::collections::HashMap::new()),
                    next_id: ::std::sync::Mutex::new(1),
                }
            }
        }
    };

    let handle_code: Vec<_> = class_defs
        .iter()
        .map(|class| generate_mod_handle_code(class, module_name))
        .collect();

    let function_manifest_entries: Vec<_> = function_defs
        .iter()
        .map(|f| {
            let name = &f.export_name;
            quote! {
                ::monty_extension_api::ExtFunctionDecl {
                    name: #name.into(),
                    is_native: true,
                }
            }
        })
        .collect();

    let function_dispatch_arms: Vec<_> = function_defs
        .iter()
        .map(|f| {
            let name = &f.export_name;
            let body = &f.dispatch_body;
            quote! { #name => #body }
        })
        .collect();

    let method_groups = group_mod_methods(&method_defs, &module_name_str);
    let method_type_arms: Vec<_> = method_groups
        .iter()
        .map(|group| {
            let type_name = &group.type_name;
            let short_name = &group.short_name;
            let arms: Vec<_> = group
                .methods
                .iter()
                .map(|m| {
                    let name = &m.export_name;
                    let body = &m.dispatch_body;
                    quote! { #name => #body }
                })
                .collect();
            quote! {
                __handle_type if __handle_type == #type_name => match method.as_str() {
                    #(#arms,)*
                    _ => {
                        return ::monty_extension_api::into_ext_result(
                            Err::<::monty_extension_api::ExtValue, _>(
                                ::monty_extension_api::ExtError::attribute_error(
                                    #short_name,
                                    method.as_str(),
                                )
                            ),
                        );
                    }
                }
            }
        })
        .collect();

    let shutdown_call = match &shutdown_ident {
        Some(ident) => quote! { #ident(self); },
        None => quote! {},
    };

    let ExtensionAttr {
        name,
        version,
        skill,
        stubs,
    } = attr;

    let type_stub_expr = if let Some(stubs) = stubs {
        quote! { ::monty_extension_api::__private::ROption::RSome((#stubs).into()) }
    } else {
        quote! { ::monty_extension_api::__private::ROption::RNone }
    };

    Ok(quote! {
        #(#module_attrs)*
        #module_vis mod #module_ident {
            #(#output_items)*

            #stored_object_enum
            #extension_struct
            #(#handle_code)*

            impl ::monty_extension_api::MontyExtension for Extension {
                fn manifest(&self) -> ::monty_extension_api::ExtManifest {
                    ::monty_extension_api::ExtManifest {
                        module_name: #name.into(),
                        functions: ::monty_extension_api::__private::RVec::from(
                            vec![#(#function_manifest_entries),*],
                        ),
                        type_stub_source: #type_stub_expr,
                        skill: (#skill).into(),
                        version: #version.into(),
                    }
                }

                fn call(
                    &self,
                    function_name: ::monty_extension_api::__private::RStr<'_>,
                    args: ::monty_extension_api::ExtArgs,
                    ctx: &::monty_extension_api::ExtContext,
                ) -> ::monty_extension_api::ExtResult {
                    match function_name.as_str() {
                        #(#function_dispatch_arms,)*
                        _ => ::monty_extension_api::into_ext_result(
                            Err::<::monty_extension_api::ExtValue, _>(
                                ::monty_extension_api::ExtError::unknown_function(
                                    #name,
                                    function_name.as_str(),
                                )
                            ),
                        ),
                    }
                }

                fn call_method(
                    &self,
                    handle: &::monty_extension_api::ExtHandle,
                    method: ::monty_extension_api::__private::RStr<'_>,
                    args: ::monty_extension_api::ExtArgs,
                    ctx: &::monty_extension_api::ExtContext,
                ) -> ::monty_extension_api::ExtResult {
                    match handle.type_name.as_str() {
                        #(#method_type_arms,)*
                        _ => {
                            let __short_type = handle
                                .type_name
                                .as_str()
                                .rsplit('.')
                                .next()
                                .unwrap_or(handle.type_name.as_str());
                            ::monty_extension_api::into_ext_result(
                                Err::<::monty_extension_api::ExtValue, _>(
                                    ::monty_extension_api::ExtError::attribute_error(
                                        __short_type,
                                        method.as_str(),
                                    )
                                ),
                            )
                        }
                    }
                }

                fn shutdown(&self) {
                    #shutdown_call
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn monty_extension_entry() -> ::monty_extension_api::ExtensionEntry {
                ::monty_extension_api::ExtensionEntry {
                    api_version: ::monty_extension_api::API_VERSION,
                    create: __monty_create_extension,
                }
            }

            extern "C" fn __monty_create_extension(
            ) -> ::monty_extension_api::MontyExtension_TO<
                'static,
                ::monty_extension_api::__private::RBox<()>,
            > {
                ::monty_extension_api::MontyExtension_TO::from_value(
                    Extension::new(),
                    ::monty_extension_api::__private::TD_Opaque,
                )
            }
        }
    })
}

/// Generates handle wrapper type and store/with/with_mut helpers for a single class.
///
/// Produces the same output as [`expand_handle_variant`] but uses `Extension` and
/// `StoredObject` as the extension type and enum names (since those are auto-generated
/// by the module-level macro).
fn generate_mod_handle_code(class: &ModClassDef, module_name: &LitStr) -> TokenStream2 {
    let class_ident = &class.ident;
    let module_name_str = module_name.value();
    let handle_ident = format_ident!("{}Handle", class_ident);
    let type_name = LitStr::new(&format!("{module_name_str}.{class_ident}"), class_ident.span());
    let expected_handle_text = LitStr::new(&format!("a {module_name_str}.{class_ident} handle"), class_ident.span());
    let short_type_name = LitStr::new(&class_ident.to_string(), class_ident.span());
    let snake = snake_case(&class_ident.to_string());
    let store_fn = format_ident!("store_{snake}");
    let with_fn = format_ident!("with_{snake}");
    let with_mut_fn = format_ident!("with_{snake}_mut");

    quote! {
        #[doc = "Typed wrapper around an `ExtHandle` for this class."]
        #[derive(Clone, Debug)]
        pub struct #handle_ident(pub ::monty_extension_api::ExtHandle);

        impl #handle_ident {
            #[doc = "Returns the borrowed ABI handle backing this typed handle."]
            #[must_use]
            pub fn as_ext_handle(&self) -> &::monty_extension_api::ExtHandle {
                &self.0
            }

            #[doc = "Returns the extension-local handle ID."]
            #[must_use]
            pub fn handle_id(&self) -> u64 {
                self.0.handle_id
            }
        }

        impl ::monty_extension_api::TryIntoExtValue for #handle_ident {
            fn try_into_ext_value(self) -> ::monty_extension_api::ExtValueResult<::monty_extension_api::ExtValue> {
                Ok(::monty_extension_api::ExtValue::Handle(self.0))
            }
        }

        impl<'a> ::monty_extension_api::FromExtValue<'a> for #handle_ident {
            fn from_ext_value(
                value: &'a ::monty_extension_api::ExtValue,
                function_name: &str,
                parameter_name: &str,
            ) -> ::monty_extension_api::ExtValueResult<Self> {
                let handle = <::monty_extension_api::ExtHandle as ::monty_extension_api::FromExtValue>::from_ext_value(
                    value,
                    function_name,
                    parameter_name,
                )?;
                if handle.type_name.as_str() != #type_name {
                    return Err(::monty_extension_api::ExtError::argument_type(
                        function_name,
                        parameter_name,
                        #expected_handle_text,
                    ));
                }
                Ok(Self(handle))
            }
        }

        impl ::monty_extension_api::MontyHandleType for #handle_ident {
            const TYPE_NAME: &'static str = #type_name;

            fn as_ext_handle(&self) -> &::monty_extension_api::ExtHandle {
                &self.0
            }

            fn into_ext_handle(self) -> ::monty_extension_api::ExtHandle {
                self.0
            }
        }

        impl Extension {
            #[doc = "Stores a new instance and returns a typed handle."]
            pub fn #store_fn(&self, value: #class_ident) -> #handle_ident {
                let mut next = self.next_id.lock().unwrap_or_else(|error| error.into_inner());
                let id = *next;
                *next += 1;
                self.objects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(id, StoredObject::#class_ident(value));
                #handle_ident(::monty_extension_api::ExtHandle {
                    type_name: #type_name.into(),
                    handle_id: id,
                    extension_id: #module_name.into(),
                })
            }

            #[doc = "Borrows a stored instance for the duration of the callback."]
            pub fn #with_fn<T>(
                &self,
                handle: &#handle_ident,
                function_name: &str,
                callback: impl FnOnce(&#class_ident) -> ::monty_extension_api::ExtValueResult<T>,
            ) -> ::monty_extension_api::ExtValueResult<T> {
                let objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
                let object = objects.get(&handle.handle_id()).ok_or_else(|| {
                    ::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )
                })?;
                match object {
                    StoredObject::#class_ident(value) => callback(value),
                    _ => Err(::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )),
                }
            }

            #[doc = "Mutably borrows a stored instance for the duration of the callback."]
            pub fn #with_mut_fn<T>(
                &self,
                handle: &#handle_ident,
                function_name: &str,
                callback: impl FnOnce(&mut #class_ident) -> ::monty_extension_api::ExtValueResult<T>,
            ) -> ::monty_extension_api::ExtValueResult<T> {
                let mut objects = self.objects.lock().unwrap_or_else(|error| error.into_inner());
                let object = objects.get_mut(&handle.handle_id()).ok_or_else(|| {
                    ::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )
                })?;
                match object {
                    StoredObject::#class_ident(value) => callback(value),
                    _ => Err(::monty_extension_api::ExtError::invalid_handle(
                        function_name,
                        #short_type_name,
                        handle.handle_id(),
                    )),
                }
            }
        }
    }
}

/// Groups method exports by their class type name for dispatch code generation.
fn group_mod_methods<'a>(methods: &'a [ModMethodDef], module_name: &str) -> Vec<ModMethodGroup<'a>> {
    let mut groups: Vec<ModMethodGroup<'a>> = Vec::new();

    for method in methods {
        let type_name_str = format!("{module_name}.{}", method.class_ident);

        if let Some(group) = groups.iter_mut().find(|g| g.type_name.value() == type_name_str) {
            group.methods.push(method);
        } else {
            groups.push(ModMethodGroup {
                type_name: LitStr::new(&type_name_str, method.class_ident.span()),
                short_name: LitStr::new(&method.class_ident.to_string(), method.class_ident.span()),
                methods: vec![method],
            });
        }
    }

    groups
}

/// Builds the dispatch body for a module-level function or method.
///
/// Module-level functions take `ext: &Extension` as their first parameter
/// (instead of `&self` in the impl-level pattern). The generated dispatch body
/// skips that first parameter and calls `function_name(self, ...)` where `self`
/// is the `Extension` instance inside the `MontyExtension` trait impl.
fn build_mod_dispatch_body(sig: &Signature, export_name: &LitStr, mode: CallMode) -> Result<TokenStream2> {
    let mut setup = Vec::new();
    let mut invoke_args = Vec::new();
    let mut arg_index = 0usize;
    let mut handle_consumed = false;

    for input in sig.inputs.iter().skip(1) {
        let FnArg::Typed(argument) = input else {
            return Err(Error::new(input.span(), "unexpected receiver in module-level function"));
        };
        let ident = extract_argument_ident(&argument.pat)?;
        let ty = &argument.ty;

        if is_ext_context(ty) {
            invoke_args.push(quote! { ctx });
            continue;
        }
        if is_ext_args(ty) {
            invoke_args.push(quote! { &args });
            continue;
        }

        if matches!(mode, CallMode::Method) && !handle_consumed {
            handle_consumed = true;
            if is_ref_ext_handle(ty) {
                invoke_args.push(quote! { handle });
            } else if is_ext_handle(ty) {
                invoke_args.push(quote! { handle.clone() });
            } else {
                setup.push(quote! {
                    let __handle_value = ::monty_extension_api::ExtValue::Handle(handle.clone());
                    let #ident: #ty = match <#ty as ::monty_extension_api::FromExtValue>::from_ext_value(
                        &__handle_value,
                        #export_name,
                        stringify!(#ident),
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return ::monty_extension_api::into_ext_result(
                                Err::<::monty_extension_api::ExtValue, _>(error),
                            );
                        }
                    };
                });
                invoke_args.push(quote! { #ident });
            }
            continue;
        }

        if let Some(inner) = option_inner_type(ty) {
            setup.push(quote! {
                let #ident: Option<#inner> = match args.optional_argument::<#inner>(
                    #arg_index,
                    #export_name,
                    stringify!(#ident),
                ) {
                    Ok(value) => value,
                    Err(error) => {
                        return ::monty_extension_api::into_ext_result(
                            Err::<::monty_extension_api::ExtValue, _>(error),
                        );
                    }
                };
            });
            invoke_args.push(quote! { #ident });
        } else {
            setup.push(quote! {
                let #ident: #ty = match args.argument::<#ty>(
                    #arg_index,
                    #export_name,
                    stringify!(#ident),
                ) {
                    Ok(value) => value,
                    Err(error) => {
                        return ::monty_extension_api::into_ext_result(
                            Err::<::monty_extension_api::ExtValue, _>(error),
                        );
                    }
                };
            });
            invoke_args.push(quote! { #ident });
        }
        arg_index += 1;
    }

    if matches!(mode, CallMode::Method) && !handle_consumed {
        return Err(Error::new(
            sig.span(),
            "method must take a handle parameter after the `ext` parameter",
        ));
    }

    let rust_name = &sig.ident;
    Ok(quote! {
        {
            #(#setup)*
            ::monty_extension_api::into_ext_result(#rust_name(self, #(#invoke_args),*))
        }
    })
}

/// Validates a module-level function or method signature.
///
/// Ensures the function is not async, has no generics, does not use `&self`,
/// and has at least one parameter (the `ext: &Extension` parameter).
fn validate_mod_function_sig(sig: &Signature) -> Result<()> {
    if sig.asyncness.is_some() {
        return Err(Error::new(
            sig.asyncness.span(),
            "async extension functions are not supported",
        ));
    }
    if sig.generics.params.iter().next().is_some() {
        return Err(Error::new(
            sig.generics.span(),
            "generic extension functions are not supported",
        ));
    }
    if sig.receiver().is_some() {
        return Err(Error::new(
            sig.inputs.span(),
            "module-level functions should take `ext: &Extension`, not `&self`",
        ));
    }
    if sig.inputs.is_empty() {
        return Err(Error::new(
            sig.span(),
            "module-level functions must take `ext: &Extension` as the first parameter",
        ));
    }
    Ok(())
}

/// Extracts the type identifier from an impl block's Self type.
///
/// Only simple paths (e.g., `impl DataFrame`) are supported. Trait impls and
/// complex type expressions are rejected with a compile error.
fn extract_impl_self_ident(imp: &ItemImpl) -> Result<Ident> {
    if imp.trait_.is_some() {
        return Err(Error::new(
            imp.span(),
            "#[monty_methods] cannot be used on trait implementations",
        ));
    }
    match &*imp.self_ty {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .ok_or_else(|| Error::new(imp.self_ty.span(), "expected a simple type name after `impl`")),
        _ => Err(Error::new(
            imp.self_ty.span(),
            "expected a simple type name after `impl`",
        )),
    }
}

/// Removes all attributes with the given name from the list.
///
/// Returns `true` if any attributes were removed.
fn strip_named_attr(attrs: &mut Vec<Attribute>, name: &str) -> bool {
    let before = attrs.len();
    attrs.retain(|a| !a.path().is_ident(name));
    attrs.len() != before
}

/// Takes a `#[function(...)]` or `#[monty_function(...)]` attribute from the list.
///
/// Returns the parsed arguments if found, and removes the attribute from the list.
/// Used for module-level free functions inside `#[monty_module] mod`.
fn take_function_export_attr(attrs: &mut Vec<Attribute>) -> Result<Option<FunctionAttr>> {
    let mut found = None;
    let mut retained = Vec::with_capacity(attrs.len());

    for attr in attrs.drain(..) {
        if is_function_attr(&attr) {
            if found.is_some() {
                return Err(Error::new(attr.span(), "duplicate function export attribute"));
            }
            found = Some(parse_function_attr(&attr)?);
        } else {
            retained.push(attr);
        }
    }

    *attrs = retained;
    Ok(found)
}

/// Takes a `#[shutdown]` or `#[monty_shutdown]` attribute from the list.
///
/// Returns `true` if found. Validates that the attribute has no arguments.
fn take_shutdown_export_attr(attrs: &mut Vec<Attribute>) -> Result<bool> {
    let mut found = false;
    let mut retained = Vec::with_capacity(attrs.len());

    for attr in attrs.drain(..) {
        if is_shutdown_attr(&attr) {
            if found {
                return Err(Error::new(attr.span(), "duplicate shutdown attribute"));
            }
            let valid =
                matches!(&attr.meta, Meta::Path(_)) || matches!(&attr.meta, Meta::List(list) if list.tokens.is_empty());
            if !valid {
                return Err(Error::new(attr.span(), "#[monty_shutdown] does not accept arguments"));
            }
            found = true;
        } else {
            retained.push(attr);
        }
    }

    *attrs = retained;
    Ok(found)
}

/// Takes a `#[method(name = "...")]` or `#[monty_method(name = "...")]` name override.
///
/// Used inside `#[monty_methods]` impl blocks for optional method name overrides.
/// When no name is specified, the function's Rust identifier is used as the export name.
fn take_method_name_override(attrs: &mut Vec<Attribute>) -> Result<Option<LitStr>> {
    let mut found = None;
    let mut retained = Vec::with_capacity(attrs.len());

    for attr in attrs.drain(..) {
        if is_method_attr(&attr) {
            if found.is_some() {
                return Err(Error::new(attr.span(), "duplicate method attribute"));
            }
            let method_attr = parse_method_attr(&attr)?;
            found = Some(method_attr.name);
        } else {
            retained.push(attr);
        }
    }

    *attrs = retained;
    Ok(found.flatten())
}
