from argparse import ArgumentParser
from json import dumps
from sys import exit
from typing import Optional

from cluster.profile.loader import ProfileUnspecifiedError, load


def main(path: Optional[str], raw: bool, silent: bool) -> str:
    try:
        value = load()
    except ProfileUnspecifiedError:
        if silent:
            exit(1)
        else:
            raise

    if path:
        for field in path.split('.'):
            if field not in value:
                raise ValueError(f"Path '{path}' not found in profile")
            value = value[field]

    if not (raw and isinstance(value, str)):
        value = dumps(value)
    return value


if __name__ == '__main__':
    parser = ArgumentParser(description=__doc__)
    parser.add_argument(
        '--path',
        help='Path to a subfield of the selected profile to print',
    )
    parser.add_argument(
        '--raw',
        action='store_true',
        help='When selecting a string-valued subfield by path,'
        + ' print the raw string value instead of a JSON-formatted string literal',
    )
    parser.add_argument(
        '--silent',
        action='store_true',
        help='Fail without printing the exception if the profile name is unspecified',
    )
    args = parser.parse_args()

    print(main(args.path, args.raw, args.silent))
