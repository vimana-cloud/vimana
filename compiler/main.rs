mod metadata;
mod wit;

use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{stdin, stdout, Read, Write};

use anyhow::{anyhow, bail, Result};
use prost::Message;
use prost_types::compiler::code_generator_response::{Feature, File};
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorProto};

use metadata::MetadataFile;
use wit::WitFile;

/// Version of the Vimana API to import.
pub(crate) const VIMANA_API_VERSION: &str = "0.0.0";
/// Version of the WASI API to import.
pub(crate) const WASI_API_VERSION: &str = "0.2.0";
/// Bitwise union of supported features.
/// https://github.com/protocolbuffers/protobuf/blob/v31.1/src/google/protobuf/compiler/code_generator.h#L96
const SUPPORTED_FEATURES: u64 = Feature::Proto3Optional as u64;

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum ProtoSyntax {
    Proto2,
    Proto3,
    Editions,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct QualifiedTypeName<'a> {
    qualifier: TypeNameQualifier<'a>,
    name: &'a str,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct TypeNameQualifier<'a> {
    package: Vec<&'a str>,
    outer_messages: Vec<&'a str>,
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
    return Ok(stdout().write_all(response.encode_to_vec().as_slice())?);
}

fn compile(request: CodeGeneratorRequest) -> Result<Vec<File>> {
    let descriptors = DescriptorMap::build(&request.proto_file)?;

    let mut wit_file: WitFile = WitFile::default();
    let mut metadata_file: MetadataFile = MetadataFile::default();

    for file_to_generate in &request.file_to_generate {
        let (file_descriptor, syntax) = descriptors.get_file(file_to_generate)?;

        // `set_or_check_server_package` *must* be invoked
        // before `compile_service` or `compile_message`.
        let package = wit_file.set_or_check_server_package(file_descriptor.package())?;

        for service_descriptor in &file_descriptor.service {
            wit_file.compile_service(service_descriptor)?;
        }

        let qualifier = TypeNameQualifier::top_level(package);
        for message_descriptor in &file_descriptor.message_type {
            wit_file.compile_message(
                message_descriptor,
                &qualifier,
                syntax.clone(),
                &descriptors,
            )?;
        }
    }

    Ok(vec![wit_file.generate()?, metadata_file.generate()?])
}

impl<'a> DescriptorMap<'a> {
    fn build(file_descriptors: &'a Vec<FileDescriptorProto>) -> Result<Self> {
        let mut descriptors = Self::default();

        for file_descriptor in file_descriptors {
            let file_name = file_descriptor.name();

            let syntax = match file_descriptor.syntax.as_ref().map(String::as_str) {
                None | Some("proto2") => ProtoSyntax::Proto2,
                Some("proto3") => ProtoSyntax::Proto3,
                Some("editions") => bail!("Editions syntax is not yet supported"),
                Some(syntax) => bail!("Unknown syntax '{syntax}' in '{file_name}'"),
            };

            let qualifier =
                TypeNameQualifier::top_level(file_descriptor.package().split('.').collect());

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
            .map(|value| value.clone())
            .ok_or_else(|| anyhow!("Malformed request contains unknown file '{filename}"))
    }

    pub(crate) fn get_message(
        &self,
        name: &QualifiedTypeName<'a>,
    ) -> Option<(&'a DescriptorProto, ProtoSyntax)> {
        self.messages.get(name).map(|value| value.clone())
    }

    pub(crate) fn get_enum(&self, name: &QualifiedTypeName<'a>) -> Option<&'a EnumDescriptorProto> {
        self.enums.get(name).map(|value| value.clone())
    }
}

impl<'a> QualifiedTypeName<'a> {
    pub(crate) fn from_path(type_path: &'a str, default_package: &Vec<&'a str>) -> Self {
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
            while let Some(part) = parts.next() {
                // Unwrapping is safe because Protobuf does not allow empty parts in a type path.
                if part.chars().next().unwrap().is_lowercase() {
                    package.push(part);
                } else {
                    outer_messages.push(part);
                    break;
                }
            }
            package
        } else {
            default_package.clone()
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
}

impl<'a> Display for QualifiedTypeName<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(
            f,
            ".{}.{}.{}",
            self.qualifier.package.join("."),
            self.qualifier.outer_messages.join("."),
            self.name
        )
    }
}

impl<'a> TypeNameQualifier<'a> {
    fn top_level(package: Vec<&'a str>) -> Self {
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

/// Convert a map into a vector of entry doubles, sorted by key.
pub(crate) fn sorted_map_entries<K: Ord, V>(map: HashMap<K, V>) -> Vec<(K, V)> {
    let mut entries: Vec<(K, V)> = map.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    entries
}

/// Convert a set into a sorted vector of values.
pub(crate) fn sorted_set_values<V: Ord>(set: HashSet<V>) -> Vec<V> {
    let mut values: Vec<V> = set.into_iter().collect();
    values.sort();
    values
}
