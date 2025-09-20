load("@bazel_skylib//lib:shell.bzl", "shell")
load("@bazel_skylib//rules:write_file.bzl", "write_file")
load("@rules_shell//shell:sh_binary.bzl", "sh_binary")

def sh_in_place(name, command, data = None):
    """
    Enable something you never want to do:
    run a shell command (with potential side effects) within the source working tree.

    This is intented only for very limited use-cases
    where such behavior is the best of only bad options,
    such as to preserve backward compatibility with the Operator SDK.

    Includes a bit of cleverness to handle Make variables like `$(location ...)`
    inside the command.
    Assumes that all Make variables are file paths (passes them to `realpath`).
    """

    data = data or []

    # Command-line arguments passed to the `sh_binary` rule.
    # Subject to Bazel's Make variable substitution.
    variables = []

    # Pieces of the command that are copied verbatim into the generated script
    # i.e. everything except the Make variables.
    non_variables = []

    # The first character after the last make variable in the command.
    end = 0

    # Partition the command into a non-overlapping and complete set of substrings
    # in two categories: variables (i.e. `$(...)` segments) and non-variables (everything else).
    # There are always exactly one more non-variable segment than there are variable segments.
    #
    # We're not actually going to iterate over every character in the command.
    # That's just an upper bound.
    # Starlark has no while loops; only iteration (but no generators or recursion).
    if len(command) == 0:
        non_variables.append("")
    for _i in range(len(command)):
        start = command.find("$(", end)
        if start == -1:
            non_variables.append(command[end:])
            break
        non_variables.append(command[end:start])
        end = command.find(")", start) + 1
        if end == 0:
            # Closing parenthesis not found.
            break
        variables.append(command[start:end])

    # Take the command segments and swap the variable ones out
    # for references that look like e.g. `${__data_0}`.
    command_segments = [
        segment
        for i in range(len(variables))
        for segment in [non_variables[i], "${{__data_{}}}".format(i)]
    ]
    command_segments.append(non_variables[-1])

    # The lines of the generated script.
    # Should look something like this:
    #     set -e
    #     __data_0="$(realpath "${1}")"
    #     __data_1="$(realpath "${2}")"
    #     cd "$BUILD_WORKING_DIRECTORY"/<caller>
    #     <command>
    content = ["set -e"] + [
        "__data_{}=\"$(realpath \"${{{}}}\")\"".format(i, i + 1)
        for i in range(len(variables))
    ] + [
        "cd \"$BUILD_WORKSPACE_DIRECTORY\"/{}".format(shell.quote(native.package_name())),
        "".join(command_segments),
    ]

    script_name = "{}.script".format(name)
    script_filename = "{}.sh".format(name)
    write_file(
        name = script_name,
        out = script_filename,
        content = content,
        is_executable = True,
    )

    sh_binary(
        name = name,
        srcs = [":{}".format(script_name)],
        args = variables,
        data = data,
    )
