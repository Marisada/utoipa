use proc_macro2::Ident;
use proc_macro_error::{abort, abort_call_site};
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Pair,
    AngleBracketedGenericArguments, Attribute, GenericArgument, PathArguments, PathSegment, Type,
    TypePath,
};

use crate::{component_type::ComponentType, Deprecated};

pub mod into_params;

pub mod component;

/// Find `#[deprecated]` attribute from given attributes. Typically derive type attributes
/// or field attributes of struct.
fn get_deprecated(attributes: &[Attribute]) -> Option<Deprecated> {
    attributes.iter().find_map(|attribute| {
        if *attribute.path.get_ident().unwrap() == "deprecated" {
            Some(Deprecated::True)
        } else {
            None
        }
    })
}

#[derive(PartialEq)]
#[cfg_attr(feature = "debug", derive(Debug))]
/// Linked list of implementing types of a field in a struct.
pub(self) struct ComponentPart<'a> {
    pub ident: &'a Ident,
    pub value_type: ValueType,
    pub generic_type: Option<GenericType>,
    pub child: Option<Box<ComponentPart<'a>>>,
}

impl<'a> ComponentPart<'a> {
    pub fn from_type(ty: &'a Type) -> ComponentPart<'a> {
        ComponentPart::from_type_path(Self::get_type_path(ty))
    }

    fn get_type_path(ty: &'a Type) -> &'a TypePath {
        match ty {
            Type::Path(path) => path,
            Type::Reference(reference) => match reference.elem.as_ref() {
                Type::Path(path) => path,
                _ => abort_call_site!("unexpected type in reference, expected Type:Path"),
            },
            Type::Group(group) => Self::get_type_path(group.elem.as_ref()),
            _ => abort_call_site!(
                "unexpected type in component part get type path, expected one of: Path, Reference, Group"
            ),
        }
    }

    fn from_type_path(type_path: &'a TypePath) -> ComponentPart<'a> {
        let segment = type_path
            .path
            .segments
            .pairs()
            .find_map(|pair| match pair {
                Pair::Punctuated(_, _) => None,
                Pair::End(segment) => Some(segment),
            })
            .unwrap();

        if segment.arguments.is_empty() {
            Self::convert(&segment.ident, segment)
        } else {
            Self::resolve_component_type(segment)
        }
    }

    // Only when type is a generic type we get to this function.
    fn resolve_component_type(segment: &'a PathSegment) -> ComponentPart<'a> {
        if segment.arguments.is_empty() {
            abort!(
                segment.ident,
                "expected at least one angle bracket argument but was 0"
            );
        };

        let mut generic_component_type = ComponentPart::convert(&segment.ident, segment);

        let generic_type = match &segment.arguments {
            PathArguments::AngleBracketed(angle_bracketed_args) => {
                // if all type arguments are lifetimes we ignore the generic type
                if angle_bracketed_args
                    .args
                    .iter()
                    .all(|arg| matches!(arg, GenericArgument::Lifetime(_)))
                {
                    None
                } else {
                    Some(ComponentPart::get_generic_arg_type(0, angle_bracketed_args))
                }
            }
            _ => abort!(
                segment.ident,
                "unexpected path argument, expected angle bracketed path argument"
            ),
        };

        generic_component_type.child = generic_type.map(ComponentPart::from_type).map(Box::new);

        generic_component_type
    }

    fn get_generic_arg_type(index: usize, args: &'a AngleBracketedGenericArguments) -> &'a Type {
        let generic_arg = args.args.iter().nth(index);

        match generic_arg {
            Some(GenericArgument::Type(generic_type)) => generic_type,
            Some(GenericArgument::Lifetime(_)) => {
                ComponentPart::get_generic_arg_type(index + 1, args)
            }
            _ => abort!(
                generic_arg,
                "expected generic argument type or generic argument lifetime"
            ),
        }
    }

    fn convert(ident: &'a Ident, segment: &PathSegment) -> ComponentPart<'a> {
        let generic_type = ComponentPart::get_generic(segment);

        Self {
            ident,
            value_type: if ComponentType(ident).is_primitive() {
                ValueType::Primitive
            } else {
                ValueType::Object
            },
            generic_type,
            child: None,
        }
    }

    fn get_generic(segment: &PathSegment) -> Option<GenericType> {
        match &*segment.ident.to_string() {
            "HashMap" | "Map" | "BTreeMap" => Some(GenericType::Map),
            "Vec" => Some(GenericType::Vec),
            "Option" => Some(GenericType::Option),
            "Cow" => Some(GenericType::Cow),
            "Box" => Some(GenericType::Box),
            "RefCell" => Some(GenericType::RefCell),
            _ => None,
        }
    }

    fn find_mut_by_ident(&mut self, ident: &'a Ident) -> Option<&mut Self> {
        match self.generic_type {
            Some(GenericType::Map) => None,
            Some(GenericType::Vec)
            | Some(GenericType::Option)
            | Some(GenericType::Cow)
            | Some(GenericType::Box)
            | Some(GenericType::RefCell) => {
                Self::find_mut_by_ident(self.child.as_mut().unwrap().as_mut(), ident)
            }
            None => {
                if ident == self.ident {
                    Some(self)
                } else {
                    None
                }
            }
        }
    }

    fn update_ident(&mut self, ident: &'a Ident) {
        self.ident = ident
    }

    /// `Any` virtual type is used when generic object is required in OpenAPI spec. Typically used
    /// with `value_type` attribute to hinder the actual type.
    fn is_any(&self) -> bool {
        &*self.ident == "Any"
    }
}

impl<'a> AsMut<ComponentPart<'a>> for ComponentPart<'a> {
    fn as_mut(&mut self) -> &mut ComponentPart<'a> {
        self
    }
}

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
enum ValueType {
    Primitive,
    Object,
}

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(PartialEq, Clone, Copy)]
enum GenericType {
    Vec,
    Map,
    Option,
    Cow,
    Box,
    RefCell,
}

/// Wrapper for [`syn::Type`] which will be resolved to [`ComponentPart`].
/// This used in `value_type` attribute to override the original field type of a struct.
#[cfg_attr(feature = "debug", derive(Debug))]
struct TypeToken(Type);

impl TypeToken {
    /// Get the [`ComponentPart`] of the [`syn::Type`].
    fn get_component_part(&self) -> ComponentPart<'_> {
        ComponentPart::from_type(&self.0)
    }
}

impl Parse for TypeToken {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(Self(input.parse::<syn::Type>()?))
    }
}

pub mod serde {
    //! Provides serde related features parsing serde attributes from types.

    use std::str::FromStr;

    use proc_macro2::{Ident, Span, TokenTree};
    use proc_macro_error::ResultExt;
    use syn::{buffer::Cursor, Attribute, Error};

    #[inline]
    fn parse_next_lit_str(next: Cursor) -> Option<(String, Span)> {
        match next.token_tree() {
            Some((tt, next)) => match tt {
                TokenTree::Punct(punct) if punct.as_char() == '=' => parse_next_lit_str(next),
                TokenTree::Literal(literal) => {
                    Some((literal.to_string().replace('\"', ""), literal.span()))
                }
                _ => None,
            },
            _ => None,
        }
    }

    #[derive(Default)]
    #[cfg_attr(feature = "debug", derive(Debug))]
    pub struct SerdeValue {
        pub skip: Option<bool>,
        pub rename: Option<String>,
    }

    impl SerdeValue {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let mut value = Self::default();

            input.step(|cursor| {
                let mut rest = *cursor;
                while let Some((tt, next)) = rest.token_tree() {
                    match tt {
                        TokenTree::Ident(ident) if ident == "skip" => value.skip = Some(true),
                        TokenTree::Ident(ident) if ident == "rename" => {
                            if let Some((literal, _)) = parse_next_lit_str(next) {
                                value.rename = Some(literal)
                            };
                        }
                        _ => (),
                    }

                    rest = next;
                }
                Ok(((), rest))
            })?;

            Ok(value)
        }
    }

    /// Attributes defined within a `#[serde(...)]` container attribute.
    #[derive(Default)]
    #[cfg_attr(feature = "debug", derive(Debug))]
    pub struct SerdeContainer {
        pub rename_all: Option<RenameRule>,
        pub tag: Option<String>,
    }

    impl SerdeContainer {
        /// Parse a single serde attribute, currently `rename_all = ...` and `tag = ...` attributes
        /// are supported.
        fn parse_attribute(&mut self, ident: Ident, next: Cursor) -> syn::Result<()> {
            match ident.to_string().as_str() {
                "rename_all" => {
                    if let Some((literal, span)) = parse_next_lit_str(next) {
                        self.rename_all = Some(
                            literal
                                .parse::<RenameRule>()
                                .map_err(|error| Error::new(span, error.to_string()))?,
                        );
                    };
                }
                "tag" => {
                    if let Some((literal, _span)) = parse_next_lit_str(next) {
                        self.tag = Some(literal)
                    }
                }
                _ => {}
            }
            Ok(())
        }

        /// Parse the attributes inside a `#[serde(...)]` container attribute.
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let mut container = Self::default();

            input.step(|cursor| {
                let mut rest = *cursor;
                while let Some((tt, next)) = rest.token_tree() {
                    if let TokenTree::Ident(ident) = tt {
                        container.parse_attribute(ident, next)?
                    }

                    rest = next;
                }
                Ok(((), rest))
            })?;

            Ok(container)
        }
    }

    pub fn parse_value(attributes: &[Attribute]) -> Option<SerdeValue> {
        attributes
            .iter()
            .find(|attribute| attribute.path.is_ident("serde"))
            .map(|serde_attribute| {
                serde_attribute
                    .parse_args_with(SerdeValue::parse)
                    .unwrap_or_abort()
            })
    }

    pub fn parse_container(attributes: &[Attribute]) -> Option<SerdeContainer> {
        attributes
            .iter()
            .find(|attribute| attribute.path.is_ident("serde"))
            .map(|serde_attribute| {
                serde_attribute
                    .parse_args_with(SerdeContainer::parse)
                    .unwrap_or_abort()
            })
    }

    #[cfg_attr(feature = "debug", derive(Debug))]
    pub enum RenameRule {
        Lower,
        Upper,
        Camel,
        Snake,
        ScreamingSnake,
        Pascal,
        Kebab,
        ScreamingKebab,
    }

    impl RenameRule {
        pub fn rename(&self, value: &str) -> String {
            match self {
                RenameRule::Lower => value.to_ascii_lowercase(),
                RenameRule::Upper => value.to_ascii_uppercase(),
                RenameRule::Camel => {
                    let mut camel_case = String::new();

                    let mut upper = false;
                    for letter in value.chars() {
                        if letter == '_' {
                            upper = true;
                            continue;
                        }

                        if upper {
                            camel_case.push(letter.to_ascii_uppercase());
                            upper = false;
                        } else {
                            camel_case.push(letter)
                        }
                    }

                    camel_case
                }
                RenameRule::Snake => value.to_string(),
                RenameRule::ScreamingSnake => Self::Snake.rename(value).to_ascii_uppercase(),
                RenameRule::Pascal => {
                    let mut pascal_case = String::from(&value[..1].to_ascii_uppercase());
                    pascal_case.push_str(&Self::Camel.rename(&value[1..]));

                    pascal_case
                }
                RenameRule::Kebab => Self::Snake.rename(value).replace('_', "-"),
                RenameRule::ScreamingKebab => Self::Kebab.rename(value).to_ascii_uppercase(),
            }
        }

        pub fn rename_variant(&self, variant: &str) -> String {
            match self {
                RenameRule::Lower => variant.to_ascii_lowercase(),
                RenameRule::Upper => variant.to_ascii_uppercase(),
                RenameRule::Camel => {
                    let mut snake_case = String::from(&variant[..1].to_ascii_lowercase());
                    snake_case.push_str(&variant[1..]);

                    snake_case
                }
                RenameRule::Snake => {
                    let mut snake_case = String::new();

                    for (index, letter) in variant.char_indices() {
                        if index > 0 && letter.is_uppercase() {
                            snake_case.push('_');
                        }
                        snake_case.push(letter);
                    }

                    snake_case.to_ascii_lowercase()
                }
                RenameRule::ScreamingSnake => {
                    Self::Snake.rename_variant(variant).to_ascii_uppercase()
                }
                RenameRule::Pascal => variant.to_string(),
                RenameRule::Kebab => Self::Snake.rename_variant(variant).replace('_', "-"),
                RenameRule::ScreamingKebab => {
                    Self::Kebab.rename_variant(variant).to_ascii_uppercase()
                }
            }
        }
    }

    impl FromStr for RenameRule {
        type Err = Error;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            [
                ("lowercase", RenameRule::Lower),
                ("UPPERCASE", RenameRule::Upper),
                ("Pascal", RenameRule::Pascal),
                ("camelCase", RenameRule::Camel),
                ("snake_case", RenameRule::Snake),
                ("SCREAMING_SNAKE_CASE", RenameRule::ScreamingSnake),
                ("kebab-case", RenameRule::Kebab),
                ("SCREAMING-KEBAB-CASE", RenameRule::ScreamingKebab),
            ]
            .into_iter()
            .find_map(|(case, rule)| if case == s { Some(rule) } else { None })
            .ok_or_else(|| {
                Error::new(
                    Span::call_site(),
                    r#"unexpected rename rule, expected one of: "lowercase", "UPPERCASE", "Pascal", "camelCase", "snake_case", "SCREAMING_SNAKE_CASE", "kebab-case", "SCREAMING-KEBAB-CASE""#,
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::serde::RenameRule;

    macro_rules! test_rename_rule {
        ( $($case:expr=> $value:literal = $expected:literal)* ) => {
            #[test]
            fn rename_all_rename_rules() {
                $(
                    let value = $case.rename($value);
                    assert_eq!(value, $expected, "expected case: {} => {} != {}", stringify!($case), $value, $expected);
                )*
            }
        };
    }

    macro_rules! test_rename_variant_rule {
        ( $($case:expr=> $value:literal = $expected:literal)* ) => {
            #[test]
            fn rename_all_rename_variant_rules() {
                $(
                    let value = $case.rename_variant($value);
                    assert_eq!(value, $expected, "expected case: {} => {} != {}", stringify!($case), $value, $expected);
                )*
            }
        };
    }

    test_rename_rule! {
        RenameRule::Lower=> "single" = "single"
        RenameRule::Upper=> "single" = "SINGLE"
        RenameRule::Pascal=> "single" = "Single"
        RenameRule::Camel=> "single" = "single"
        RenameRule::Snake=> "single" = "single"
        RenameRule::ScreamingSnake=> "single" = "SINGLE"
        RenameRule::Kebab=> "single" = "single"
        RenameRule::ScreamingKebab=> "single" = "SINGLE"

        RenameRule::Lower=> "multi_value" = "multi_value"
        RenameRule::Upper=> "multi_value" = "MULTI_VALUE"
        RenameRule::Pascal=> "multi_value" = "MultiValue"
        RenameRule::Camel=> "multi_value" = "multiValue"
        RenameRule::Snake=> "multi_value" = "multi_value"
        RenameRule::ScreamingSnake=> "multi_value" = "MULTI_VALUE"
        RenameRule::Kebab=> "multi_value" = "multi-value"
        RenameRule::ScreamingKebab=> "multi_value" = "MULTI-VALUE"
    }

    test_rename_variant_rule! {
        RenameRule::Lower=> "Single" = "single"
        RenameRule::Upper=> "Single" = "SINGLE"
        RenameRule::Pascal=> "Single" = "Single"
        RenameRule::Camel=> "Single" = "single"
        RenameRule::Snake=> "Single" = "single"
        RenameRule::ScreamingSnake=> "Single" = "SINGLE"
        RenameRule::Kebab=> "Single" = "single"
        RenameRule::ScreamingKebab=> "Single" = "SINGLE"

        RenameRule::Lower=> "MultiValue" = "multivalue"
        RenameRule::Upper=> "MultiValue" = "MULTIVALUE"
        RenameRule::Pascal=> "MultiValue" = "MultiValue"
        RenameRule::Camel=> "MultiValue" = "multiValue"
        RenameRule::Snake=> "MultiValue" = "multi_value"
        RenameRule::ScreamingSnake=> "MultiValue" = "MULTI_VALUE"
        RenameRule::Kebab=> "MultiValue" = "multi-value"
        RenameRule::ScreamingKebab=> "MultiValue" = "MULTI-VALUE"
    }

    #[test]
    fn test_serde_rename_rule_from_str() {
        for s in [
            "lowercase",
            "UPPERCASE",
            "Pascal",
            "camelCase",
            "snake_case",
            "SCREAMING_SNAKE_CASE",
            "kebab-case",
            "SCREAMING-KEBAB-CASE",
        ] {
            s.parse::<RenameRule>().unwrap();
        }
    }
}