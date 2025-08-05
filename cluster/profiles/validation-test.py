"""Validate cluster profiles against a JSON schema."""

from unittest import TestCase, main
from os.path import join as joinPath

from jsonschema import validate
from yaml import safe_load as loadYaml

from cluster.profiles.load import PROFILES_PATH, load as loadProfile

SCHEMA_PATH = joinPath('cluster', 'profiles', 'schema.yaml')


class ProfilesValidationTest(TestCase):
    def test_raw(self):
        """Test that the raw `profiles.yaml` file is valid."""

        with open(SCHEMA_PATH, 'r') as file:
            schema = loadYaml(file)
        with open(PROFILES_PATH, 'r') as file:
            profiles = loadYaml(file)

        validate(instance=profiles, schema=schema)

    def test_normalized(self):
        """
        Test all profiles are valid after being loaded and normalized.

        If this test fails but `test_raw` succeeds, that indicates a problem with `loadProfile`.
        """

        with open(SCHEMA_PATH, 'r') as file:
            schema = loadYaml(file)
        with open(PROFILES_PATH, 'r') as file:
            rawProfiles = loadYaml(file)

        for name in rawProfiles.keys():
            profiles = {name: loadProfile(name)}
            import sys

            print(profiles, file=sys.stderr)
            validate(instance=profiles, schema=schema)


if __name__ == '__main__':
    main()
