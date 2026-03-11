mod metadata;
mod wit;

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{Read, Write, stdin, stdout};

use anyhow::{Result, anyhow, bail};
use prost::Message;
use prost_types::compiler::code_generator_response::{Feature, File};
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorProto};

use metadata::MetadataFile;
use wit::WitFile;

/// Bitwise union of supported features.
/// https://github.com/protocolbuffers/protobuf/blob/v31.1/src/google/protobuf/compiler/code_generator.h#L96
const SUPPORTED_FEATURES: u64 = Feature::Proto3Optional as u64;

/// Plugin parameters that can be set via `--vimana_opt` command-line options.
pub(crate) struct PluginParameters {
    /// Ignore group-typed fields in proto2 instead of failing.
    pub(crate) ignore_groups: bool,

    /// Ignore required fields in proto2 instead of failing.
    pub(crate) ignore_required: bool,

    /// Support empty Protobuf messages
    /// by inserting an unused value in the generated record type definition.
    /// but only when used exclusively for request / response types.
    pub(crate) allow_empty: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum ProtoSyntax {
    Proto2,
    Proto3,
    // TODO: Add support for editions.
}

/// A fully-qualified type name for either a message or enum type.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct QualifiedTypeName<'a> {
    /// The "namespace" in which this type lives.
    qualifier: TypeNameQualifier<'a>,
    /// Short name of the type (Protobuf syntax).
    name: &'a str,
}

/// Protobuf effectively allows 2 layers of namespacing for types:
/// the package, and (optionally) nesting messages.
/// Both messages and enums can be defined within an outer message,
/// which can itself be defined within an outer message, and so on.
/// The structure captures both layers of namespacing as a unit.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct TypeNameQualifier<'a> {
    package: ProtoPackage<'a>,
    outer_messages: Vec<&'a str>,
}

/// Represents a Protobuf package name (e.g. `some.package`).
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct ProtoPackage<'a> {
    path: Vec<&'a str>,
}

/// Keeps track of all the relevant descriptors from a [request](CodeGeneratorRequest).
#[derive(Default)]
pub(crate) struct DescriptorMap<'a> {
    /// Mapping from filenames to file descriptors.
    files: HashMap<String, (&'a FileDescriptorProto, ProtoSyntax)>,
    /// Mapping from fully-qualified message type names to message descriptors
    /// and the Protobuf syntax that applies to each descriptor.
    messages: HashMap<QualifiedTypeName<'a>, (&'a DescriptorProto, ProtoSyntax)>,
    /// Mapping from fully-qualified enum type names to enum descriptors.
    enums: HashMap<QualifiedTypeName<'a>, &'a EnumDescriptorProto>,
}

impl PluginParameters {
    /// Parse a string of comma-separated parameter names into a [`PluginParameters`] object.
    /// Return an error if any of the parameter names are unrecognized.
    fn parse(parameters: &str) -> Result<Self> {
        let mut parameters: HashSet<&str> = parameters
            .split(',')
            .filter(|parameter| !parameter.is_empty())
            .collect();
        let result = Self {
            ignore_groups: parameters.remove("ignore-groups"),
            ignore_required: parameters.remove("ignore-required"),
            allow_empty: parameters.remove("allow-empty"),
        };
        if !parameters.is_empty() {
            bail!(
                "Unknown parameters: {:?}",
                parameters.iter().collect::<Vec<_>>()
            );
        }
        Ok(result)
    }
}

fn main() -> Result<()> {
    // Read and parse the entire input from stdin.
    // If an error occurs here, exit with a failure status.
    let mut buf: Vec<u8> = Vec::new();
    stdin().read_to_end(&mut buf)?;
    let request: CodeGeneratorRequest = CodeGeneratorRequest::decode(buf.as_slice())?;

    // Generate a response.
    // If an error occurs after this point,
    // write it as an error on the generated response, but exit normally.
    let mut response = CodeGeneratorResponse {
        file: Vec::new(),
        error: None,
        supported_features: Some(SUPPORTED_FEATURES),
    };
    match compile(request) {
        Ok(files) => response.file.extend(files),
        Err(error) => response.error = Some(error.to_string()),
    }

    // Write the response to stdout.
    Ok(stdout().write_all(response.encode_to_vec().as_slice())?)
}

fn compile(request: CodeGeneratorRequest) -> Result<Vec<File>> {
    let parameters = PluginParameters::parse(request.parameter())?;
    let descriptors = DescriptorMap::build(&request.proto_file)?;

    let mut wit_file: WitFile = WitFile::new(&descriptors, &parameters);
    let mut metadata_file: MetadataFile = MetadataFile::new(&descriptors, &parameters);
    let main_package: OnceCell<ProtoPackage> = OnceCell::new();

    for file_to_generate in &request.file_to_generate {
        let (file_descriptor, syntax) = descriptors.get_file(file_to_generate)?;

        let package_qualifier = TypeNameQualifier::top_level(set_or_check_main_package(
            file_descriptor.package(),
            &main_package,
        )?);

        for service_descriptor in &file_descriptor.service {
            wit_file.compile_service(service_descriptor, &package_qualifier)?;
            metadata_file.compile_service(service_descriptor, &package_qualifier)?;
        }

        for message_descriptor in &file_descriptor.message_type {
            wit_file.compile_message(message_descriptor, &package_qualifier, syntax)?;
        }
    }

    Ok(if let Some(package) = main_package.get() {
        let mut results = wit_file.generate(package)?;
        results.push(metadata_file.generate()?);
        results
    } else {
        Vec::default()
    })
}

/// Set the main package namespace.
/// If the namespace has already been set (from a different file),
/// check that it's consistent with what was previously set.
fn set_or_check_main_package<'a>(
    package: &'a str,
    main_package: &OnceCell<ProtoPackage<'a>>,
) -> Result<ProtoPackage<'a>> {
    let package = ProtoPackage::parse(package);
    let existing_package = main_package.get_or_init(|| package.clone());
    if &package != existing_package {
        bail!("Conflicting packages: {existing_package} and {package}")
    }
    Ok(package)
}

impl<'a> DescriptorMap<'a> {
    fn build(file_descriptors: &'a Vec<FileDescriptorProto>) -> Result<Self> {
        let mut descriptors = Self::default();

        for file_descriptor in file_descriptors {
            let file_name = file_descriptor.name();

            let syntax = match file_descriptor.syntax.as_deref() {
                None | Some("proto2") => ProtoSyntax::Proto2,
                Some("proto3") => ProtoSyntax::Proto3,
                Some("editions") => bail!("Editions syntax is not yet supported"),
                Some(syntax) => bail!("Unknown syntax '{syntax}' in '{file_name}'"),
            };

            let qualifier =
                TypeNameQualifier::top_level(ProtoPackage::parse(file_descriptor.package()));

            for message_type in &file_descriptor.message_type {
                descriptors.insert_message(message_type, qualifier.clone(), syntax);
            }
            for enum_type in &file_descriptor.enum_type {
                descriptors.insert_enum(enum_type, qualifier.clone());
            }

            descriptors
                .files
                .insert(String::from(file_name), (file_descriptor, syntax));
        }

        Ok(descriptors)
    }

    fn insert_message(
        &mut self,
        descriptor: &'a DescriptorProto,
        qualifier: TypeNameQualifier<'a>,
        syntax: ProtoSyntax,
    ) {
        let name = descriptor.name();

        // Recursively add all nested messages and enums.
        let nested_qualifier = qualifier.nested(name);
        for nested_message in &descriptor.nested_type {
            self.insert_message(nested_message, nested_qualifier.clone(), syntax);
        }
        for nested_enum in &descriptor.enum_type {
            self.insert_enum(nested_enum, nested_qualifier.clone());
        }

        self.messages
            .insert(qualifier.into_type(name), (descriptor, syntax));
    }

    fn insert_enum(
        &mut self,
        enum_descriptor: &'a EnumDescriptorProto,
        qualifier: TypeNameQualifier<'a>,
    ) {
        self.enums
            .insert(qualifier.into_type(enum_descriptor.name()), enum_descriptor);
    }

    fn get_file(&self, filename: &String) -> Result<(&'a FileDescriptorProto, ProtoSyntax)> {
        self.files
            .get(filename)
            .copied()
            .ok_or_else(|| anyhow!("Malformed request contains unknown file '{filename}"))
    }

    pub(crate) fn get_message(
        &self,
        name: &QualifiedTypeName<'a>,
    ) -> Option<(&'a DescriptorProto, ProtoSyntax)> {
        self.messages.get(name).copied()
    }

    pub(crate) fn get_enum(&self, name: &QualifiedTypeName<'a>) -> Option<&'a EnumDescriptorProto> {
        self.enums.get(name).copied()
    }
}

impl<'a> QualifiedTypeName<'a> {
    pub(crate) fn from_path(type_path: &'a str, qualifier_context: &TypeNameQualifier<'a>) -> Self {
        let mut parts = type_path.split('.');

        // The final part is the short name
        // (e.g. `some-message` for a message with Protobuf name `.package.SomeMessage`).
        // Unwrapping is safe because `split` always yields at least 1 element.
        let name = parts.next_back().unwrap();

        // If the path starts with a leading period, it includes an explicit package.
        // Otherwise, assume the same package namespace as the server.
        let mut outer_messages: Vec<&'a str> = Vec::new();
        let package = if type_path.starts_with('.') {
            // Skip the first (empty) part due to the leading period.
            parts.next();
            // Distinguish package parts from nested message parts
            // based on the capitalization of the first character
            // (packages start with a lowercase character, messages uppercase).
            let mut package: Vec<&'a str> = Vec::new();
            for part in parts.by_ref() {
                // Unwrapping is safe because Protobuf does not allow empty parts in a type path.
                if part.chars().next().unwrap().is_lowercase() {
                    package.push(part);
                } else {
                    outer_messages.push(part);
                    break;
                }
            }
            ProtoPackage::new(package)
        } else {
            // Right now, this just assumes that any non-fully-qualified type name
            // must refer to a top-level type in the same Protobuf package
            // as the qualifier context.
            // TODO: A proper search of the DescriptorMap using C++-style scoping rules.
            // github.com/protocolbuffers/protobuf/blob/v33.1/src/google/protobuf/descriptor.proto#L296-L300
            qualifier_context.package.clone()
        };

        // Any remaining parts must be outer nesting messages.
        for part in parts {
            outer_messages.push(part);
        }

        QualifiedTypeName {
            qualifier: TypeNameQualifier {
                package,
                outer_messages,
            },
            name,
        }
    }

    /// Assuming this type name represents a message type,
    /// return the qualifier for types nested within that message.
    fn subqualifier(&self) -> TypeNameQualifier<'a> {
        self.qualifier.nested(self.name)
    }
}

impl<'a> Display for QualifiedTypeName<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, ".{}", self.qualifier.package)?;
        for outer_message in &self.qualifier.outer_messages {
            write!(f, ".{}", outer_message)?;
        }
        write!(f, ".{}", self.name)
    }
}

impl<'a> TypeNameQualifier<'a> {
    fn top_level(package: ProtoPackage<'a>) -> Self {
        Self {
            package,
            outer_messages: Vec::default(),
        }
    }

    fn nested(&self, name: &'a str) -> Self {
        let mut outer_messages = self.outer_messages.clone();
        outer_messages.push(name);
        Self {
            package: self.package.clone(),
            outer_messages,
        }
    }

    fn r#type(&self, name: &'a str) -> QualifiedTypeName<'a> {
        QualifiedTypeName {
            qualifier: self.clone(),
            name,
        }
    }

    fn into_type(self, name: &'a str) -> QualifiedTypeName<'a> {
        QualifiedTypeName {
            qualifier: self,
            name,
        }
    }
}

impl<'a> ProtoPackage<'a> {
    fn new(path: Vec<&'a str>) -> Self {
        Self { path }
    }

    fn parse(name: &'a str) -> Self {
        Self::new(name.split('.').collect())
    }

    fn top_level_qualifier(&self) -> TypeNameQualifier<'a> {
        TypeNameQualifier::top_level(self.clone())
    }
}

impl<'a> Display for ProtoPackage<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.path.join("."))
    }
}

/// Convert a map into a vector of entry doubles, sorted by key.
pub(crate) fn into_sorted_map_entries<K: Ord, V>(map: HashMap<K, V>) -> Vec<(K, V)> {
    let mut entries: Vec<(K, V)> = map.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    entries
}

/// Convert a set into a sorted vector of values.
pub(crate) fn into_sorted_set_values<V: Ord>(set: HashSet<V>) -> Vec<V> {
    let mut values: Vec<V> = set.into_iter().collect();
    values.sort();
    values
}

/// Convert a set into a sorted vector of values (borrowed version).
pub(crate) fn sorted_set_values<V: Ord>(set: &HashSet<V>) -> Vec<&V> {
    let mut values: Vec<&V> = set.iter().collect();
    values.sort();
    values
}
