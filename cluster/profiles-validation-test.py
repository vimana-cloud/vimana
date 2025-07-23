"""Validate cluster profiles against a JSON schema."""

from argparse import ArgumentParser

from jsonschema import validate
from yaml import safe_load as load


def main(profilesPath: str, schemaPath: str):
    with open(profilesPath, 'r') as file:
        profiles = load(file)
    with open(schemaPath, 'r') as file:
        schema = load(file)
    validate(instance=profiles, schema=schema)


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('profiles', help='Path to the profiles YAML file')
    parser.add_argument(
        '--schema',
        required=True,
        metavar='PATH',
        help='Path to the profiles JSON schema (as a YAML file)',
    )
    args = parser.parse_args()

    main(args.profiles, args.schema)
