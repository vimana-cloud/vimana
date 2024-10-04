use std::collections::HashMap;
use std::io::Result;

use prost_types::compiler::code_generator_response::File;
use prost_types::compiler::{CodeGeneratorRequest, CodeGeneratorResponse};
use prost_types::field_descriptor_proto::Type;
use prost_types::{DescriptorProto, FileDescriptorProto};

use common::{error, required, run_protoc_plugin};

fn main() -> Result<()> {
    return run_protoc_plugin(compile, emit);
}

fn compile(request: CodeGeneratorRequest) -> Result<Vec<ServiceWit>> {
    // Mapping from all filenames to file descriptors.
    let mut file_descriptors: HashMap<String, &FileDescriptorProto> = HashMap::new();
    // Mapping from all fully-qualified message type names to message descriptors.
    let mut descriptors: HashMap<String, &DescriptorProto> = HashMap::new();
    for proto_file in &request.proto_file {
        for message_type in &proto_file.message_type {
            descriptors.insert(required(message_type.name.clone())?, message_type);
        }
        file_descriptors.insert(required(proto_file.name.clone())?, proto_file);
    }

    // 
    let mut packages: HashMap<String, Package> = HashMap::new();

    // Process each file.
    // One WIT file is generated per service.
    for file_to_generate in &request.file_to_generate {
        let file_descriptor = required(file_descriptors.get(file_to_generate))?;
        let package_name = required(file_descriptor.package.as_ref())?;

        for service_descriptor in &file_descriptor.service {
            let mut service = Service::default();
            service.name = service_descriptor
                .name
                .clone()
                .ok_or(error("Service has empty name"))?;
            let mut streaming: bool = false;

            for method_descriptor in &service_descriptor.method {
                let request_type = required(method_descriptor.input_type.as_ref())?;
                let response_type = required(method_descriptor.output_type.as_ref())?;

                let client_streaming = method_descriptor.client_streaming.unwrap_or(false);
                let server_streaming = method_descriptor.server_streaming.unwrap_or(false);
                if client_streaming || server_streaming {
                    streaming = true;
                }

                match method_descriptor.options.as_ref() {
                    Some(options) => {
                        for option in &options.uninterpreted_option {
                            // TODO
                        }
                    }
                    None => (),
                }

                if streaming && service.http_routes.len() > 0 {
                    return Err(error(
                        "Service cannot include both streaming RPCs and JSON transcoding.",
                    ));
                }

                service.type_imports.push(String::from(response_type));
                insert_types_recursively(&mut descriptors, request_type, package)?;
                insert_types_recursively(&mut descriptors, response_type, package)?;
            }
            package.services.push(service);
        }
    }

    return Ok(services);
}

fn insert_types_recursively(
    descriptors: &mut HashMap<String, &DescriptorProto>,
    type_name: &str,
    package: &mut Package,
) -> Result<()> {
    if package.messages.contains_key(type_name) {
        Ok(())
    } else {
        let descriptor = required(descriptors.get(type_name))?;
        let mut message = Message { fields: Vec::new() };
        for field in &descriptor.field {
            message.fields.push(Field {
                name: required(field.name.clone())?,
                field_type: convert_type(Type::try_from(required(field.r#type)?)?) as i32,
            })
        }
        // Insert the current message before recursing to avoid infinite looping.
        package.messages.insert(String::from(type_name), message);
        for field in &descriptor.field {
            insert_types_recursively(descriptors, field.name(), package)?;
        }
        Ok(())
    }
}

fn convert_type(r#type: Type) -> FieldType {
    match r#type {
        Type::Double => FieldType::Float64,
        Type::Float => FieldType::Float32,
        Type::Int64 => FieldType::S64,
        Type::Uint64 => FieldType::U64,
        Type::Int32 => FieldType::S32,
        Type::Fixed64 => FieldType::S64,
        Type::Fixed32 => FieldType::S32,
        Type::Bool => FieldType::Bool,
        Type::String => FieldType::String,
        Type::Group => FieldType::Record,   // TODO
        Type::Message => FieldType::Record, // TODO
        Type::Bytes => FieldType::List,     // TODO
        Type::Uint32 => FieldType::U32,
        Type::Enum => FieldType::Enum, // TODO
        Type::Sfixed32 => FieldType::S32,
        Type::Sfixed64 => FieldType::S64,
        Type::Sint32 => FieldType::S32,
        Type::Sint64 => FieldType::S64,
    }
}

fn emit(wit: Vec<ServiceWit>) -> Result<CodeGeneratorResponse> {
    let mut response: CodeGeneratorResponse = CodeGeneratorResponse::default();
    for (package_name, package) in services {
        if package.messages.len() > 0 {
            let mut buffer = String::new();
            buffer.push_str("package ");
            buffer.push_str(&package_name);
            buffer.push_str("\n\ninterface %type {\n\n");

            for (type_name, message) in package.messages {
                buffer.push_str("\n  record ");
                print_kebab_case(&type_name, &mut buffer);
                buffer.push_str(" {");

                if message.fields.len() > 0 {
                    buffer.push('\n'); // Pretty print.
                    for field in message.fields {
                        // TODO
                    }
                }

                buffer.push_str("  }\n");
            }
            buffer.push_str("}\n");
            response.file.push(File {
                name: Some(String::from(format!("{}/%type.wit", package_name))),
                insertion_point: None,
                content: Some(buffer),
                generated_code_info: None,
            })
        }

        for service in package.services {
            let mut buffer = String::new();
            write!(buffer, "package ");
            buffer.push_str(&package_name);
            buffer.push_str(";\n\nworld ");
            print_kebab_case(&service.name, &mut buffer);
            buffer.push_str(" {");

            if service.methods.len() > 0 {
                buffer.push('\n'); // Pretty print.
                for method in service.methods {
                    buffer.push_str("\n  export ");
                    print_kebab_case(&method.name, &mut buffer);
                    buffer.push_str(": func(context: context, request: ");
                    print_kebab_case(&method.request_type, &mut buffer);
                    buffer.push_str(") -> ");
                    print_kebab_case(&method.response_type, &mut buffer);
                    buffer.push_str(";\n");
                }
            }

            buffer.push_str("}\n");

            response.file.push(File {
                name: Some(format!("{}/{}.wit", package_name, service.name)),
                insertion_point: None,
                content: Some(buffer),
                generated_code_info: None,
            })
        }
    }

    Ok(response)
}

fn print_kebab_case(name: &str, content: &mut String) {
    content.push_str(&name[..1].to_lowercase());
    print_kebab_case_continue(&name[1..], content);
}

fn print_kebab_case_continue(name: &str, content: &mut String) {
    match name.find(char::is_uppercase) {
        Some(index) => {
            content.push_str(&name[..index]);
            content.push('-');
            content.push_str(&name[index..index + 1].to_lowercase());
            print_kebab_case_continue(&name[index + 1..], content)
        }
        None => content.push_str(name),
    }
}

pub struct Package {
    pub services: Vec<ServiceWit>,

    pub types: Vec<Type>,
}

// Represents a WIT file with a single world representing a service.
pub struct ServiceWit {
    // WIT package, of the form `<domain>:<service-name>@<version>`.
    // Both the domain and fully-qualified service name are derived from the real values as follows:
    // 1. Periods `.` are converted to dashes `-`.
    // 2. CamelCase is converted to kebab-case.
    // E.g. `api-example-com:com-example-api-hello-world@1.2.3`
    pub package: String,

    // Name of the service in kebab-case, e.g. `hello-world`.
    // This is always a suffix of the service name in `package`.
    pub name: String,

    pub functions: Vec<Function>,

    // Mapping from fully-qualified type name to record type descriptors.
    // Has information on all request and response record types for all methods in this service,
    // as well as any transitively nested types therein.
    pub types: HashMap<String, RecordType>,

    pub serializers: Vec<Serializer>,
}

// Represents a function of WIT interface.
struct Function {
    pub name: String,
}

// Represents a WIT record type.
pub struct RecordType {}

pub struct Serializer {
    // Fully-qualified type name of the type for which to generate serialization logic.
    pub type_name: String,
}
