""" IPAM CNI plugin emulator.

This should generally act like the [`host-local`][1] plugin.

[1]: https://www.cni.dev/plugins/current/ipam/host-local/
"""

from ipaddress import IPv4Network, IPv6Network, ip_network
from json import dump, load
from os import getenv, path
from sqlite3 import Cursor, IntegrityError, connect
from sys import stdin, stdout

# Save state between invocations in this SQLite database.
DB_PATH = path.join(getenv('TMPDIR', '/tmp'), 'test-ipam-host-local.db')

# CNI API parameters:
CNI_VERSION = '1.0.0'
NETWORK_NAME = 'kindnet'
IPAM_TYPE = 'host-local'

def add(db: Cursor, container: str, ipam):
    ranges = ipam['ranges']
    assert isinstance(ranges, list) \
        and len(ranges) == 1 and len(ranges[0]) == 1 \
        and 'subnet' in ranges[0][0], \
            f'Expected a single address range, got {ranges}'
    cidr = ip_network(ranges[0][0]['subnet'])

    # Find an unused address the slow way.
    for i, address in enumerate(cidr.hosts()):
        try:
            # Dodge race conditions:
            # find the first available address by iteratively attempting insertion.
            # There should be a uniqueness constraint on both columns.
            db.execute('INSERT OR FAIL INTO address VALUES (?, ?)', (container, str(address)))
            db.connection.commit()
            break
        except IntegrityError:
            # This could happen either because the container already has an IP address,
            # or the IP address is already allocated to some other container.
            # Essentially, we want to fail on the former and continue iterating on the latter.
            #
            # The latter condition is considered normal, and could occur repeatedly in the loop,
            # so only check for the former condition once in a while, for performance
            # (arbitrarily, every hundred times, starting on the first failure).
            if i % 100 == 0:
                existing = \
                    db.execute('SELECT address FROM address WHERE container = ?', (container,)) \
                        .fetchone()
                assert existing is None, f'Existing entry for {repr(container)}: {repr(existing)}'
    else:
        raise RuntimeError(f'Ran out of addresses in range {str(cidr)}!')

    dump({
        'cniVersion': CNI_VERSION,
        'ips': [{
            # Return the address with a subnet mask.
            'address': f'{address}/{cidr.prefixlen}',
            # TODO: Figure out how the gateway is relevant to Vimana's IPAM.
            'gateway': 'TODO',
        }],
    }, stdout)

def delete(db: Cursor, container: str, ipam):
    result = db.execute('DELETE FROM address WHERE container = ?', (container,))
    assert result.rowcount == 1, f'No entry for {repr(container)}'
    db.connection.commit()

actions = {
    'ADD': add,
    'DEL': delete,
}

def main():
    command = getenv('CNI_COMMAND')
    container = getenv('CNI_CONTAINERID')
    config = load(stdin)
    version = config['cniVersion']
    network = config['name']
    assert 'ipam' in config, 'Expected \'ipam\' in configuration'
    ipam = config['ipam']
    ipamType = ipam['type']
    dataDir = ipam['dataDir']

    # Check that the runtime is behaving as expected.
    # Not all of these checks are strictly necessary; change them if they stop making sense.
    assert command in actions, f'Unknown command: {command}'
    assert version == CNI_VERSION, f'Unexpected CNI version: {version}'
    assert network == NETWORK_NAME, f'Unexpected network name: {network}'
    assert ipamType == IPAM_TYPE, f'Unexpected IPAM type: {ipamType}'
    assert dataDir == '/run/cni-ipam-state', f'Unexpected IPAM data directory: {dataDir}'

    # Persist all allocations to a database in the temporary directory,
    # so non-colliding addresses can be allocated across test runs,
    # but the pool gets regularly reset (on reboot).
    cursor = connect(DB_PATH).cursor()
    cursor.execute('CREATE TABLE IF NOT EXISTS address(container TEXT PRIMARY KEY, address TEXT)')
    cursor.execute('CREATE UNIQUE INDEX IF NOT EXISTS address_index ON address(address)')

    return actions[command](cursor, container, ipam)

if __name__ == '__main__':
    main()