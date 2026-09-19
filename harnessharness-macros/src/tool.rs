use proc_macro2::TokenStream;
use quote::quote;
use syn::Attribute;
use syn::Error;
use syn::Expr;
use syn::FnArg;
use syn::Ident;
use syn::ItemFn;
use syn::Lit;
use syn::Meta;
use syn::Pat;
use syn::PatType;
use syn::ReturnType;
use syn::Type;
use syn::spanned::Spanned;

const CONTEXT_TYPE_NAME: &str = "ToolContext";
const RESULT_TYPE_NAME: &str = "Result";

struct InputField<'a> {
    docs: Vec<&'a Attribute>,
    ident: &'a Ident,
    ty: &'a Type,
}

enum Parameter<'a> {
    Input(InputField<'a>),
    Context,
}

pub fn expand(attribute: TokenStream, function: ItemFn) -> Result<TokenStream, Error> {
    check_is_supported(&attribute, &function)?;
    let parameters = parse_parameters(&function)?;
    let description = parse_description(&function)?;
    let visibility = &function.vis;
    let ident = &function.sig.ident;
    let name = ident.to_string();
    let docs = filter_docs(&function.attrs);
    let input_struct = expand_input_struct(&parameters);
    let inner_function = expand_inner_function(&function);
    let call = expand_call(&function, &parameters);
    Ok(quote! {
        #(#docs)*
        #[allow(non_upper_case_globals)]
        #visibility static #ident: ::harnessharness::tool::ToolDef = {
            #input_struct
            #inner_function
            fn __input_schema() -> ::harnessharness::__private::serde_json::Value {
                ::harnessharness::__private::input_schema::<__ToolInput>()
            }
            #call
            ::harnessharness::tool::ToolDef::new(#name, #description, __input_schema, __call)
        };
    })
}

fn expand_input_struct(parameters: &[Parameter]) -> TokenStream {
    let fields = parameters.iter().filter_map(|parameter| match parameter {
        Parameter::Input(InputField { docs, ident, ty }) => Some(quote! {
            #(#docs)*
            #ident: #ty,
        }),
        Parameter::Context => None,
    });
    quote! {
        #[derive(
            ::harnessharness::__private::serde::Deserialize,
            ::harnessharness::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::harnessharness::__private::serde", deny_unknown_fields)]
        #[schemars(crate = "::harnessharness::__private::schemars")]
        struct __ToolInput {
            #(#fields)*
        }
    }
}

fn expand_inner_function(function: &ItemFn) -> TokenStream {
    let mut inner_function = function.clone();
    inner_function.vis = syn::Visibility::Inherited;
    inner_function.attrs.retain(|attr| !is_doc(attr));
    for argument in inner_function.sig.inputs.iter_mut() {
        if let FnArg::Typed(pat_type) = argument {
            pat_type.attrs.retain(|attr| !is_doc(attr));
        }
    }
    quote! { #inner_function }
}

fn expand_call(function: &ItemFn, parameters: &[Parameter]) -> TokenStream {
    let ident = &function.sig.ident;
    let arguments = parameters.iter().map(|parameter| match parameter {
        Parameter::Input(InputField { ident, .. }) => quote! { input.#ident },
        Parameter::Context => quote! { context },
    });
    let await_suffix = match function.sig.asyncness {
        Some(_) => quote! { .await },
        None => quote! {},
    };
    let conversion = match returns_result(&function.sig.output) {
        true => quote! {
            match output {
                Ok(value) => __serialize(&value),
                Err(error) => Err(::std::string::ToString::to_string(&error)),
            }
        },
        false => quote! { __serialize(&output) },
    };
    quote! {
        fn __serialize(
            value: &impl ::harnessharness::__private::serde::Serialize,
        ) -> Result<::harnessharness::tool::ToolOutput, String> {
            ::harnessharness::tool::ToolOutput::json(value).map_err(|error| error.to_string())
        }
        fn __call<'a>(
            input: ::harnessharness::__private::serde_json::Value,
            context: &'a ::harnessharness::tool::ToolContext,
        ) -> ::harnessharness::tool::ToolFuture<'a> {
            Box::pin(async move {
                let _ = context;
                let input: __ToolInput =
                    ::harnessharness::__private::serde_json::from_value(input)
                        .map_err(|error| error.to_string())?;
                let output = #ident(#(#arguments),*) #await_suffix;
                #conversion
            })
        }
    }
}

fn check_is_supported(attribute: &TokenStream, function: &ItemFn) -> Result<(), Error> {
    if !attribute.is_empty() {
        return Err(Error::new(attribute.span(), "`#[tool]` takes no arguments"));
    }
    let generics = &function.sig.generics;
    if !generics.params.is_empty() || generics.where_clause.is_some() {
        return Err(Error::new(
            generics.span(),
            "`#[tool]` functions cannot be generic",
        ));
    }
    Ok(())
}

fn parse_parameters(function: &ItemFn) -> Result<Vec<Parameter<'_>>, Error> {
    function.sig.inputs.iter().map(parse_parameter).collect()
}

fn parse_parameter(argument: &FnArg) -> Result<Parameter<'_>, Error> {
    let pat_type = match argument {
        FnArg::Typed(pat_type) => pat_type,
        FnArg::Receiver(receiver) => {
            return Err(Error::new(
                receiver.span(),
                "`#[tool]` functions cannot take `self`; implement `Tool` for stateful tools",
            ));
        }
    };
    match is_context(&pat_type.ty) {
        true => Ok(Parameter::Context),
        false => parse_input_field(pat_type).map(Parameter::Input),
    }
}

fn parse_input_field(pat_type: &PatType) -> Result<InputField<'_>, Error> {
    let ident = match pat_type.pat.as_ref() {
        Pat::Ident(pat_ident) if pat_ident.subpat.is_none() => &pat_ident.ident,
        pattern => {
            return Err(Error::new(
                pattern.span(),
                "`#[tool]` parameters must be plain identifiers",
            ));
        }
    };
    Ok(InputField {
        docs: filter_docs(&pat_type.attrs),
        ident,
        ty: &pat_type.ty,
    })
}

fn parse_description(function: &ItemFn) -> Result<String, Error> {
    let lines = function
        .attrs
        .iter()
        .filter_map(parse_doc_line)
        .collect::<Vec<String>>();
    let description = lines.join("\n").trim().to_string();
    match description.is_empty() {
        true => Err(Error::new(
            function.sig.ident.span(),
            "`#[tool]` requires a doc comment; it becomes the tool description",
        )),
        false => Ok(description),
    }
}

fn parse_doc_line(attr: &Attribute) -> Option<String> {
    let Meta::NameValue(name_value) = &attr.meta else {
        return None;
    };
    if !is_doc(attr) {
        return None;
    }
    match &name_value.value {
        Expr::Lit(expr_lit) => match &expr_lit.lit {
            Lit::Str(line) => Some(line.value().trim().to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn filter_docs(attrs: &[Attribute]) -> Vec<&Attribute> {
    attrs.iter().filter(|attr| is_doc(attr)).collect()
}

fn is_doc(attr: &Attribute) -> bool {
    attr.path().is_ident("doc")
}

fn is_context(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => is_named(&reference.elem, CONTEXT_TYPE_NAME),
        _ => false,
    }
}

fn returns_result(output: &ReturnType) -> bool {
    match output {
        ReturnType::Type(_, ty) => is_named(ty, RESULT_TYPE_NAME),
        ReturnType::Default => false,
    }
}

fn is_named(ty: &Type, name: &str) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == name)
}
