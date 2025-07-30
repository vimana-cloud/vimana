use std::fmt::{Display, Formatter, Result as FmtResult};

use heck::ToKebabCase;

use metadata_proto::work::runtime::Field;

const INDENTATION: &str = "  ";

/// Boilerplate for a `String`-based newtype
/// that represents both a Protobuf name and a WIT version of that name.
/// Initialized with the Protobuf version,
/// it implements `Display` using the WIT version,
/// but `Debug` using the Protobuf version.
macro_rules! proto_wit_name {
    ($(#[$meta:meta])* $type_name:ident, $argument:ident => $to_wit:block,) => {
        $(#[$meta])*
        #[derive(Debug)]
        struct $type_name(String);

        impl $type_name {
            fn from_proto<T: Into<String>>(name: T) -> Self {
                Self(name.into())
            }

            fn to_wit(&self) -> String {
                let $argument = &self.0;
                $to_wit
            }
        }

        impl Display for $type_name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
                formatter.write_str(&self.to_wit())
            }
        }
    };

    // Assume conversion via [heck::ToKebabCase] by default.
    ($(#[$meta:meta])* $type_name:ident,) => {
        proto_wit_name!(
            $(#[$meta])*
            $type_name,
            name => { name.to_kebab_case() },
        );
    };
}

proto_wit_name!(
    /// A Protobuf package name and a namespaced WIT package name.
    PackageName,
    name => { name.replace('.', ":") },
);

proto_wit_name!(
    /// A protobuf service name and a WIT interface name.
    InterfaceName,
);

proto_wit_name!(
    /// A protobuf RPC name and a WIT function name.
    FunctionName,
);

proto_wit_name!(
    /// A protobuf message name and a WIT record name.
    RecordName,
);

proto_wit_name!(
    /// A protobuf message field name and a WIT record field name.
    RecordFieldName,
);

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#top-level-items
/// Vimana generates one WIT file per package,
/// containing one interface per service,
/// and a nested package containing the types.
struct Package {
    /// Full package name, including optional version.
    full_name: FullPackageName,

    /// Interfaces within the package.
    interfaces: Vec<Interface>,
}

/// A full package name, including optional version.
struct FullPackageName {
    /// Package name (including namespaces) without a version.
    name: PackageName,

    /// Package version.
    version: Option<String>,
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#item-interface
/// An interface that is a member of a [package](Package),
/// which corresponds to a Protobuf service.
struct Interface {
    /// Interface ID (name).
    id: InterfaceName,

    functions: Vec<Function>,

    records: Vec<Record>,
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#item-interface
/// A function that is a member of an [interface](Interface),
/// which corresponds to a Protobuf RPC.
struct Function {
    id: FunctionName,

    /// Parameter names and types.
    parameters: Vec<Parameter>,

    /// Result type.
    result: QualifiedRecordName,
}

/// A parameter to a [function](Function).
/// Vimana's generated RPC methods always take record-typed parameters.
struct Parameter {
    /// Parameter name.
    /// Represented as a simple string because there is no Protobuf equivalent.
    id: String,

    /// Parameter type.
    r#type: QualifiedRecordName,
}

/// A fully-qualified record name, including the full package name.
struct QualifiedRecordName {
    /// Record name.
    name: RecordName,

    /// Unversioned package name.
    package: PackageName,
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#item-record-bag-of-named-fields
struct Record {
    /// Record name.
    id: RecordName,

    /// List of record fields.
    fields: Vec<RecordField>,
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#item-record-bag-of-named-fields
struct RecordField {
    /// Record field name.
    id: RecordFieldName,

    r#type: Field,
}

impl Package {
    fn new(name: PackageName, version: Option<String>) -> Self {
        Self {
            full_name: FullPackageName { name, version },
            interfaces: Vec::new(),
        }
    }

    fn add_interface(&mut self, interface: Interface) {
        self.interfaces.push(interface)
    }
}

impl Interface {
    fn new(id: InterfaceName) -> Self {
        Self {
            id,
            functions: Vec::new(),
            records: Vec::new(),
        }
    }

    fn add_function(&mut self, function: Function) {
        self.functions.push(function)
    }
}

impl Function {
    fn new(id: FunctionName, parameters: Vec<Parameter>, result: QualifiedRecordName) -> Self {
        Self {
            id,
            parameters,
            result,
        }
    }
}

impl Parameter {
    fn new(id: String, r#type: QualifiedRecordName) -> Self {
        Self { id, r#type }
    }
}

impl QualifiedRecordName {
    fn new(name: RecordName, package: PackageName) -> Self {
        Self { name, package }
    }
}

impl Display for Package {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "\npackage {};\n", self.full_name)?;
        for interface in &self.interfaces {
            write!(formatter, "\n{}", interface)?;
        }
        Ok(())
    }
}

impl Display for FullPackageName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}", self.name)?;
        if let Some(version) = &self.version {
            formatter.write_str("@")?;
            formatter.write_str(version)?;
        }
        Ok(())
    }
}

impl Display for Interface {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "interface {} {{\n", self.id)?;
        for function in &self.functions {
            write!(formatter, "\n{}{};\n", INDENTATION, function)?;
        }
        for record in &self.records {
            // Indentation and the trailing newline are handled in [Record::fmt].
            write!(formatter, "\n{}", record)?;
        }
        formatter.write_str("}\n")?;
        Ok(())
    }
}

impl Display for Function {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}: func(", self.id)?;
        let mut parameters = self.parameters.iter();
        if let Some(parameter) = parameters.next() {
            write!(formatter, "{}", parameter)?;
            for parameter in parameters {
                write!(formatter, ", {}", parameter)?;
            }
        }
        write!(formatter, ") -> {}", self.result)?;
        Ok(())
    }
}

impl Display for Parameter {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}: {}", self.id, self.r#type)
    }
}

impl Display for QualifiedRecordName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}:{}", self.package, self.name)
    }
}

impl Display for Record {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}record {} {{", INDENTATION, self.id)?;
        let mut fields = self.fields.iter();
        if let Some(field) = fields.next() {
            write!(
                formatter,
                "\n{}{}{}: {}",
                INDENTATION,
                INDENTATION,
                field.id,
                record_wit_type(&field.r#type),
            )?;
            for field in fields {
                write!(
                    formatter,
                    ",\n{}{}{}: {}",
                    INDENTATION,
                    INDENTATION,
                    field.id,
                    record_wit_type(&field.r#type),
                )?;
            }
        }
        write!(formatter, "\n{}}}", INDENTATION)
    }
}

fn record_wit_type(field: &Field) -> String {
    String::from("TODO")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::env::var;
    use std::io::{pipe, PipeReader, Write};
    use std::mem::drop;
    use std::process::Command;

    #[test]
    #[ignore = "Multi-part namespaces not yet supported"]
    fn empty_package_no_version() {
        let package = Package::new(PackageName::from_proto("foo.bar.baz"), None);

        validate(
            &package,
            r#"
package foo:bar:baz;
"#,
        );
    }

    #[test]
    fn empty_package_with_version() {
        let package = Package::new(
            PackageName::from_proto("host.vimana"),
            Some("0.2.4-fersher".into()),
        );

        validate(
            &package,
            r#"
package host:vimana@0.2.4-fersher;
"#,
        );
    }

    #[test]
    #[ignore = "Multi-part namespaces not yet supported"]
    fn interface_with_functions() {
        let mut interface = Interface::new(InterfaceName::from_proto("SomeService"));
        interface.add_function(Function::new(
            FunctionName::from_proto("SomeRPCMethod"),
            vec![],
            QualifiedRecordName::new(
                RecordName::from_proto("ReturnType"),
                PackageName::from_proto("some.other.package"),
            ),
        ));
        let mut package = Package::new(PackageName::from_proto("foo"), None);
        package.add_interface(interface);

        validate(
            &package,
            r#"
package foo;

interface some-service {

  some-rpc-method: func() -> some:other:package:return-type;
}
"#,
        );
    }

    fn validate(package: &Package, expected_wit: &str) {
        // First check that the WIT is literally what we're expecting.
        assert_eq!(package.to_string(), expected_wit);

        // Then use `wasm-tools` to validate it.
        let wasm_tools_path = var("WASMTOOLS").expect("Must set `WASMTOOLS` env var for test");
        let output = Command::new(wasm_tools_path)
            .args(["component", "wit", "--wasm", "--all-features"])
            .stdin(str_pipe(expected_wit))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "WIT validation failed.\n{}",
            String::from_utf8(output.stderr).unwrap()
        );
    }

    /// Create a new unnamed pipe to feed a string to a command's standard input.
    fn str_pipe(input: &str) -> PipeReader {
        let (reader, mut writer) = pipe().unwrap();
        writer.write_all(input.as_bytes()).unwrap();
        drop(writer); // Flush the pipe.
        reader
    }
}
