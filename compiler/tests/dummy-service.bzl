load("@bazel_skylib//rules:write_file.bzl", "write_file")

def dummy_service(name, package, imports, type):
    """
    Provide a Protobuf service definition in a particular package
    that makes use of a particular type.

    The compiler only bothers to emit codec metadata for types
    that are actually referenced (possibly transitively)
    by one of the methods in one of the services being compiled.
    That means that the test proto definitions used for the conformance tests
    would be ignored, since they are not normally referenced in any service definitions.

    This convenience macro generates a Protobuf source file with a simple service called `Dummy`,
    with a single method called `Dummy` which uses the given type
    as both its request and response types.
    Including this in the Protobuf sources of the compiler
    will cause it to emit code metadata for that type.
    """
    write_file(
        name = name,
        out = "{}.proto".format(name),
        content = [
            "syntax = \"proto3\";",
            "package {};".format(package),
        ] + [
            "import \"{}\";".format(_import)
            for _import in imports
        ] + [
            "service Dummy {",
            "  rpc Dummy({}) returns ({}) {{}}".format(type, type),
            "}",
        ],
    )
