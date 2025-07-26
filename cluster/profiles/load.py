from os.path import join as joinPath
from typing import Dict

from yaml import safe_load as loadYaml

PROFILES_PATH = joinPath('cluster', 'profiles', 'profiles.yaml')


def load(name: str) -> Dict[str, object]:
    """
    Load and normalize a profile by name.

    Normalizing involves populating optional fields with default values.
    """

    with open(PROFILES_PATH, 'r') as file:
        profiles = loadYaml(file)
    if name not in profiles:
        raise ValueError(f'Profile {name} not found')
    profile = profiles[name]

    if 'gcp' in profile:
        _populateDefaultsGcp(profile['gcp'])
    if 'aws' in profile:
        _populateDefaultsAws(profile['aws'])
    if 'azure' in profile:
        _populateDefaultsAzure(profile['azure'])

    return profile


def _populateDefaultsGcp(gcp: Dict[str, object]):
    # Use the official published node images for GCP by default.
    if 'image-project' not in gcp:
        gcp['image-project'] = 'vimana-node-images'
    if 'image-family' not in gcp:
        gcp['image-family'] = 'vimana'
    pass


def _populateDefaultsAws(gcp: Dict[str, object]):
    pass


def _populateDefaultsAzure(gcp: Dict[str, object]):
    pass
