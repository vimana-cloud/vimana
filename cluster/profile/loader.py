from os import getenv
from typing import Dict

from python.runfiles import Runfiles
from yaml import safe_load as loadYaml

runfiles = Runfiles.Create()

# Path to the profiles configuration, which must be in the runfiles.
PROFILES_PATH = runfiles.Rlocation('_main/cluster/profile/profiles.yaml')


class ProfileUnspecifiedError(RuntimeError):
    """An exception thrown when the user fails to configure a non-empty profile name."""


def name() -> str:
    """Return the name of the selected profile."""
    # The name of the profile as configured by the user with the `--//cluster/profile` flag.
    # This must be set as an environment variable,
    # generally by reading the "Make" variable from the flag's toolchain.
    return getenv('PROFILE', '')


def load() -> Dict[str, object]:
    """
    Load and normalize a profile by name.

    The name is passed via the `PROFILE` environment variable.

    Normalizing involves populating optional fields with default values.
    """

    profileName = name()
    if not profileName:
        raise ProfileUnspecifiedError('Pass a profile name with `--//cluster/profile`')

    with open(PROFILES_PATH, 'r') as file:
        profiles = loadYaml(file)
    if profileName not in profiles:
        raise ValueError(f"Profile '{profileName}' not found")
    profile = profiles[profileName]

    if 'gcp' in profile:
        _populateDefaultsGcp(profile['gcp'])
    if 'aws' in profile:
        _populateDefaultsAws(profile['aws'])
    if 'azure' in profile:
        _populateDefaultsAzure(profile['azure'])

    return profile


def _populateDefaultsGcp(gcp: Dict[str, object]):
    pass


def _populateDefaultsAws(gcp: Dict[str, object]):
    pass


def _populateDefaultsAzure(gcp: Dict[str, object]):
    pass
