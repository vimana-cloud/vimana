"""
Module extension to set up hermetic CC and Rust toolchains
to cross-compile from the various supported platforms to other supported platforms.
"""

load("@hermetic_cc_toolchain//toolchain:defs.bzl", zig_toolchains = "toolchains")
load("@rules_rust//rust:repositories.bzl", "rust_repository_set")
load("@rules_rust//rust/private:repository_utils.bzl", "toolchain_repository_hub")

# Some of the features used in this repo require a nightly Rust compiler.
RUST_NIGHTLY_DATE = "2026-01-01"
RUST_VERSION = "nightly/{}".format(RUST_NIGHTLY_DATE)
RUST_EDITION = "2024"

# The set of all target platforms (which is a superset of all exec platforms)
# that are supported for Rust compilation.
X86_64_LINUX_GNU = struct(
    triple = "x86_64-unknown-linux-gnu",
    constraints = [
        "@platforms//cpu:x86_64",
        "@platforms//os:linux",
        "@zig_sdk//libc:unconstrained",
    ],
)
AARCH64_LINUX_GNU = struct(
    triple = "aarch64-unknown-linux-gnu",
    constraints = [
        "@platforms//cpu:aarch64",
        "@platforms//os:linux",
        "@zig_sdk//libc:unconstrained",
    ],
)
X86_64_LINUX_MUSL = struct(
    triple = "x86_64-unknown-linux-musl",
    constraints = [
        "@platforms//cpu:x86_64",
        "@platforms//os:linux",
        "@zig_sdk//libc:musl",
    ],
)
AARCH64_LINUX_MUSL = struct(
    triple = "aarch64-unknown-linux-musl",
    constraints = [
        "@platforms//cpu:aarch64",
        "@platforms//os:linux",
        "@zig_sdk//libc:musl",
    ],
)
X86_64_MACOS = struct(
    triple = "x86_64-apple-darwin",
    constraints = [
        "@platforms//cpu:x86_64",
        "@platforms//os:macos",
    ],
)
AARCH64_MACOS = struct(
    triple = "aarch64-apple-darwin",
    constraints = [
        "@platforms//cpu:aarch64",
        "@platforms//os:macos",
    ],
)
WASM32_WASI = struct(
    triple = "wasm32-wasip2",
    constraints = [
        "@platforms//cpu:wasm32",
        "@platforms//os:wasi",
    ],
)

# Execution platforms.
EXEC_PLATFORMS = {
    "linux": ["amd64", "arm64"],
    "macos": ["amd64", "arm64"],
}

def _rust_repository_set(target, exec):
    return rust_repository_set(
        name = "rust.{}.{}".format(exec.triple, target.triple),
        edition = RUST_EDITION,
        exec_triple = exec.triple,
        extra_target_triples = {target.triple: target.constraints},
        versions = [RUST_VERSION],
        register_toolchain = False,
    )

def _impl(mctx):
    # Call `toolchains` unconditionally so we can access `zig_sdk`
    # as both the root module or a dependency.
    # By default, `hermetic_cc_toolchain` only sets up `zig_sdk` for the root module.
    # https://github.com/uber/hermetic_cc_toolchain/blob/v4.1.0/toolchain/ext.bzl#L33
    repos = zig_toolchains(exec_platforms = EXEC_PLATFORMS)

    # Create all Rust cross-compilation toolchain repositories directly,
    # rather than via `rust.repository_set()` tags in MODULE.bazel,
    # which are only processed for the root module.
    #
    # The following table conveys which target platforms (columns)
    # can be compiled from which execution platforms (rows).
    #
    #     |    | XLm | ALm | XLg | ALg | XM | AM | WW |
    #     | XL |  ✔  |  ✔  |  ✔  |     |    |    | ✔  |
    #     | AL |  ✔  |  ✔  |     |  ✔  |    |    | ✔  |
    #     | XM |  ✔  |  ✔  |     |     | ✔  | ✔  | ✔  |
    #     | AM |  ✔  |  ✔  |     |     | ✔  | ✔  | ✔  |
    #
    # - XLm = x86_64-linux-musl
    # - ALm = aarch64-linux-musl
    # - XLg = x86_64-linux-gnu
    # - ALg = aarch64-linux-gnu
    # - XM = x86_64-macos
    # - AM = aarch64-macos
    # - WW = wasm32-wasip2
    toolchains_info = (
        _rust_repository_set(target = X86_64_LINUX_MUSL, exec = X86_64_LINUX_GNU) |
        _rust_repository_set(target = X86_64_LINUX_MUSL, exec = AARCH64_LINUX_GNU) |
        _rust_repository_set(target = X86_64_LINUX_MUSL, exec = X86_64_MACOS) |
        _rust_repository_set(target = X86_64_LINUX_MUSL, exec = AARCH64_MACOS) |
        _rust_repository_set(target = AARCH64_LINUX_MUSL, exec = X86_64_LINUX_GNU) |
        _rust_repository_set(target = AARCH64_LINUX_MUSL, exec = AARCH64_LINUX_GNU) |
        _rust_repository_set(target = AARCH64_LINUX_MUSL, exec = X86_64_MACOS) |
        _rust_repository_set(target = AARCH64_LINUX_MUSL, exec = AARCH64_MACOS) |
        _rust_repository_set(target = X86_64_LINUX_GNU, exec = X86_64_LINUX_GNU) |
        _rust_repository_set(target = AARCH64_LINUX_GNU, exec = AARCH64_LINUX_GNU) |
        _rust_repository_set(target = X86_64_MACOS, exec = X86_64_MACOS) |
        _rust_repository_set(target = X86_64_MACOS, exec = AARCH64_MACOS) |
        _rust_repository_set(target = AARCH64_MACOS, exec = X86_64_MACOS) |
        _rust_repository_set(target = AARCH64_MACOS, exec = AARCH64_MACOS) |
        _rust_repository_set(target = WASM32_WASI, exec = X86_64_LINUX_GNU) |
        _rust_repository_set(target = WASM32_WASI, exec = AARCH64_LINUX_GNU) |
        _rust_repository_set(target = WASM32_WASI, exec = X86_64_MACOS) |
        _rust_repository_set(target = WASM32_WASI, exec = AARCH64_MACOS)
    )

    toolchain_repository_hub(
        name = "vimana_toolchains",
        toolchain_names = toolchains_info.keys(),
        toolchain_labels = {
            name: info["tools_toolchain_label"]
            for name, info in toolchains_info.items()
        },
        toolchain_types = {
            name: info["toolchain_type"]
            for name, info in toolchains_info.items()
        },
        target_settings = {
            name: info["target_settings"]
            for name, info in toolchains_info.items()
        },
        exec_compatible_with = {
            name: info["exec_compatible_with"]
            for name, info in toolchains_info.items()
        },
        target_compatible_with = {
            name: info["target_compatible_with"]
            for name, info in toolchains_info.items()
        },
    )

    public_repos = repos.public + ["vimana_toolchains"]
    if mctx.root_module_has_non_dev_dependency:
        return mctx.extension_metadata(
            root_module_direct_deps = public_repos,
            root_module_direct_dev_deps = [],
        )
    else:
        return mctx.extension_metadata(
            root_module_direct_deps = [],
            root_module_direct_dev_deps = public_repos,
        )

toolchains = module_extension(implementation = _impl)
