"""
Upgrade all Bazel module, Rust crate, Python package, or Go module dependencies
to the latest versions available
from Bazel Central Registry, crates.io, PyPI, and pkg.go.dev, respectively.

If no particular languages are specified, upgrade dependencies for all languages by default.
"""

from argparse import ArgumentParser
from json import loads as loadsJson
from os import chdir, getenv
from os.path import join as joinPath
from os.path import realpath
from re import compile as compileRegex
from typing import Dict

from packaging.requirements import Requirement, SpecifierSet
from tomlkit import dump as dumpToml
from tomlkit import load as loadToml

from dev.lib.util import console, requestOrDie, runOrDie, step

# Absolute paths to tool binaries.
# `RUNFILES_DIR` is set when invoked via `bazel build`.
# `..` is the parent for external repo data dependencies when invoked via `bazel run`.
RUNFILES_DIR = getenv('RUNFILES_DIR', '..')
BUILDOZER_PATH = realpath(
    joinPath(RUNFILES_DIR, 'buildifier_prebuilt+', 'buildozer', 'buildozer')
)
GO_PATH = realpath(
    joinPath(RUNFILES_DIR, 'rules_go+', 'go', 'tools', 'go_bin_runner', 'bin', 'go')
)


def main(bazel: bool = True, rust: bool = True, python: bool = True, go: bool = True):
    # Move to the top level of the Git Repo for this function.
    # The source repo becomes the working directory.
    # Source files can be mutated, in contrast to Bazel's usual hermeticity.
    # https://bazel.build/docs/user-manual#running-executables
    chdir(getenv('BUILD_WORKSPACE_DIRECTORY'))

    if bazel:
        with step('Upgrading Bazel module dependencies'):
            upgradeBazelModules()
    if rust:
        with step('Upgrading Rust crate dependencies'):
            upgradeRustCrates()
    if python:
        with step('Upgrading Python package dependencies'):
            upgradePythonPackages()
    if go:
        with step('Upgrading Go module dependencies'):
            upgradeGoModules()


def upgradeBazelModules():
    # Read the name and version of each `bazel_dep` in `MODULE.bazel`.
    result = runOrDie(
        [BUILDOZER_PATH, 'print name version', '//MODULE.bazel:%bazel_dep']
    )

    # Create a list of Buildozer commands to run a batch of updates together.
    updates = []
    for line in result.stdout.splitlines():
        name, currentVersion = line.split()

        # Buildozer prints `(missing)` if the `bazel_dep` has no version.
        # It's probably a `git_override` dependency. Just skip these.
        if currentVersion == '(missing)':
            console.print(
                f'Skipping Bazel module [bold]{name}[/bold] with missing version'
            )
            continue

        # Get the latest version from the registry.
        latestVersion = requestOrDie(
            'GET',
            f'https://raw.githubusercontent.com/bazelbuild/bazel-central-registry/main/modules/{name}/metadata.json',
        ).json()['versions'][-1]

        if currentVersion != latestVersion:
            updates.append(
                f'replace version {currentVersion} {latestVersion}|//MODULE.bazel:{name}\n'
            )
            printUpdate(name, currentVersion, latestVersion, 'green')

    # Execute all updates with a single Buildozer command.
    runOrDie([BUILDOZER_PATH, '-f', '-'], input=''.join(updates))


def upgradeRustCrates():
    with open('Cargo.toml', 'r') as cargoFile:
        cargo = loadToml(cargoFile)
    dependencies = cargo['dependencies']

    for name in dependencies.keys():
        # Rust dependencies are expressed as either a simple version string,
        # or a JSON object with the field 'version'.
        if isinstance(dependencies[name], dict):
            upgradeRustCrate(name, dependencies[name], 'version')
        else:
            upgradeRustCrate(name, dependencies, name)

    with open('Cargo.toml', 'w') as cargoFile:
        dumpToml(cargo, cargoFile)


def upgradeRustCrate(crateName, versionObject: Dict[str, any], versionKey: str):
    currentVersion = versionObject[versionKey]

    # Compute the directory for the package's metadata on crates.io.
    # https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
    indexDirectory = (
        str(len(crateName))
        if len(crateName) <= 2
        else f'3/{crateName[0]}'
        if len(crateName) == 3
        else f'{crateName[0:2]}/{crateName[2:4]}'
    )

    # Get the latest version from crates.io.
    latestVersion = loadsJson(
        requestOrDie(
            'GET',
            f'https://index.crates.io/{indexDirectory}/{crateName}',
        ).content.splitlines()[-1]
    )['vers']

    if currentVersion != latestVersion:
        versionObject[versionKey] = latestVersion
        printUpdate(crateName, currentVersion, latestVersion, 'yellow')


def upgradePythonPackages():
    updatedLines = []

    with open('requirements.txt', 'r') as requirementsFile:
        for line in requirementsFile:
            # Remove any comments, and leading or trailing whitespace.
            requirement = line
            if '#' in requirement:
                requirement = requirement[: requirement.index('#')]
            requirement = requirement.strip()

            # If there's anything left, parse it as a requirement.
            if requirement:
                requirement = Requirement(line)

                # YAGNI
                assert requirement.url is None, (
                    f"Cannot handle requirement URL '{requirement.url}'"
                )

                # Get the latest version from PyPI.
                latestVersion = requestOrDie(
                    'GET',
                    f'https://pypi.org/pypi/{requirement.name}/json',
                ).json()['info']['version']

                if not requirement.specifier.contains(latestVersion):
                    oldSpecifier = requirement.specifier
                    requirement.specifier = SpecifierSet(f'=={latestVersion}')
                    updatedLines.append(f'{requirement}\n')
                    printUpdate(
                        requirement.name,
                        str(oldSpecifier),
                        str(requirement.specifier),
                        'blue',
                    )
                    continue

            # If the line did not express a requirement (i.e. it was a blank line or comment),
            # or if the version specifier already includes the latest version,
            # copy the line verbatim to the updated file.
            updatedLines.append(line)

    with open('requirements.txt', 'w') as requirementsFile:
        for line in updatedLines:
            requirementsFile.write(line)


def upgradeGoModules():
    goDependencyRegex = compileRegex(r'^\t([^\s]+) ([^\s]+)(| // indirect)\n$')

    # Manually parse the `go.mod` file line by line, building updated contents for it.
    # Perhaps there's a more robust solution using the `go` binary,
    # but using `go list -m` and `go get` was often "too smart"
    # and would run into weird errors.
    updatedLines = []
    with open('go.mod', 'r') as goModFile:
        # Whether we're currently parsing inside a `require ( ... )` block.
        inRequire = False

        for line in goModFile:
            if line == 'require (\n':
                assert not inRequire
                inRequire = True
            elif line == ')\n':
                assert inRequire
                inRequire = False
            elif inRequire and (match := goDependencyRegex.match(line)) is not None:
                path = match.group(1)
                currentVersion = match.group(2)
                indirect = match.group(3)

                # Encode the module path for the Go module proxy protocol.
                # Uppercase letters must be replaced with '!' followed by the lowercase equivalent.
                # https://go.dev/ref/mod#goproxy-protocol
                encodedPath = ''.join(
                    f'!{char.lower()}' if char.isupper() else char for char in path
                )

                # Get the latest version using the Go module proxy protocol.
                latestVersion = requestOrDie(
                    'GET',
                    f'https://proxy.golang.org/{encodedPath}/@latest',
                ).json()['Version']

                if currentVersion != latestVersion:
                    updatedLines.append(f'\t{path} {latestVersion}{indirect}\n')
                    printUpdate(path, currentVersion, latestVersion, 'cyan')
                    continue

            updatedLines.append(line)

    with open('go.mod', 'w') as goModFile:
        for line in updatedLines:
            goModFile.write(line)

    runOrDie([GO_PATH, 'mod', 'tidy'])


def printUpdate(name: str, oldVersion: str, newVersion: str, color: str):
    """
    Print a user-facing message to stderr indicating that a package is being updated.
    Color is used to indicate the type of package
    (green for Bazel, yellow for Rust, blue for Python, cyan for Go).
    """
    console.print(
        f'[{color}]{name}[/{color}] [bold]{oldVersion}[/bold] ➜ [bold]{newVersion}[/bold]'
    )


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('--bazel', action='store_true', help='upgrade Bazel modules')
    parser.add_argument('--rust', action='store_true', help='upgrade Rust crates')
    parser.add_argument('--python', action='store_true', help='upgrade Python packages')
    parser.add_argument('--go', action='store_true', help='upgrade Go modules')
    args = parser.parse_args()

    # If no languages were specified,
    # upgrade dependencies for all languages.
    if not any([args.bazel, args.rust, args.python, args.go]):
        args.rust = args.bazel = args.python = args.go = True

    main(bazel=args.bazel, rust=args.rust, python=args.python, go=args.go)
