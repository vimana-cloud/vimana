load("@rules_rust//rust:defs.bzl", "rust_library")

def _kebab_to_snake(s):
    return s.replace('-', '_')

def _wit_bindgen_rust_impl(ctx):
    world = ctx.attr.world or ctx.label.name
    output = ctx.actions.declare_file(_kebab_to_snake(world) + ".rs")
    outputs = [output]
    arguments = ["rust", "--world", world, "--out-dir", output.dirname]
    for src in ctx.files.srcs:
        arguments.append(src.path)
    ctx.actions.run(
        inputs = ctx.files.srcs,
        outputs = outputs,
        executable = ctx.executable._wit_bindgen_bin,
        arguments = arguments,
    )
    return [DefaultInfo(files = depset(outputs))]


wit_bindgen_rust = rule(
    implementation = _wit_bindgen_rust_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".wit"], doc = "WIT source files."),
        "world": attr.string(
            doc = "World to generate bindings for. Default is the target name.",
        ),
        "_wit_bindgen_bin": attr.label(
            # TODO: Use wit-bindgen-cli crate dependency
            #   instead of checking in the binary.
            # https://github.com/bazelbuild/rules_rust/discussions/2786
            default = "//util:wit-bindgen",
            allow_files = True,
            executable = True,
            cfg = "exec",
        ),
    },
)

def rust_wit_bindgen(name, srcs, world = None):
    """
    Given a `.wit` file and a world name,
    generate Rust bindings and compile it to a rust library called `name`.

    Default for `world` is `name`.

    To get the generated Rust sources directly, use [wit_bindgen_rust].
    """
    if world == None:
        world = name
    src_name = "~" + name + "-src"
    wit_bindgen_rust(
        name = src_name,
        srcs = srcs,
        world = world,
    )
    rust_library(
        name = name,
        srcs = [":" + src_name],
        crate_name = _kebab_to_snake(world),
    )
