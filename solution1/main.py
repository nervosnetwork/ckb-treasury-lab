import contextlib
import json
import os
import pathlib
import subprocess
import sys
import time
import typing

import pyckb


def bash(cmd: str) -> subprocess.CompletedProcess[bytes]:
    print(f'$ {cmd}')
    return subprocess.run(cmd, shell=True, check=True)


def bash_output(cmd: str) -> subprocess.CompletedProcess[bytes]:
    print(f'$ {cmd}')
    return subprocess.run(cmd, shell=True, check=True, capture_output=True)


@contextlib.contextmanager
def cd(dst: str) -> typing.Generator[None, typing.Any, None]:
    # Context manager to temporarily change the current working directory.
    cwd = os.getcwd()
    os.chdir(dst)
    yield
    os.chdir(cwd)


def smoking():
    subprocess.run('pm k ckbm ckbr', shell=True)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('rm -rf data ckb ckb-cli pk1 pk2')
    with cd('/home/ubuntu/.ckb-cli/keystore'):
        bash('rm -rf UTC*')
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('ln -s ~/app/ckb/bin/ckb .')
        bash('ln -s ~/app/ckb/bin/ckb-cli .')
        bash('echo 0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc > pk1')
        bash('echo 0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d > pk2')
        bash('./ckb-cli account import --privkey-path pk1')
        bash('./ckb-cli account import --privkey-path pk2')
        bash('PMNAME=ckbm pm c ./ckb run --indexer')
        bash('PMNAME=ckbr pm c ./ckb miner')
    time.sleep(20)
    pyckb.config.upgrade(pyckb.config.develop.rpc.url)
    pyckb.config.current = pyckb.config.develop
    gods = [
        pyckb.wallet.Wallet(0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc),
        pyckb.wallet.Wallet(0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d),
    ]
    user = [pyckb.wallet.Wallet(i) for i in range(1, 3)]
    gods[0].transfer(user[0].script, gods[0].capacity() // 2)
    gods[1].transfer(user[1].script, gods[1].capacity() // 2)
    time.sleep(20)
    for u in gods + user:
        u.dao_deposit(1000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/sdk'):
        bash('bun run src/cli/index.ts devnet-scripts --out ../deployment/devnet.json')
        bash(' '.join([
            'bun run src/cli/index.ts deploy',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--out ../deployment/devnet.json',
        ]))
        time.sleep(20)
        bash(' '.join([
            'bun run src/cli/index.ts create-config',
            '--config ../deployment/devnet.json',
            '--write ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--minimal-proposal-capacity 200',
            '--yes-threshold 100',
            '--minimal-vote-amount 100',
            '--vote-duration 10',
            '--vote-window 10',
            '--challenge-time 10',
        ]))
        time.sleep(20)
        proposal = bash_output(' '.join([
            'bun run src/cli/index.ts create-proposal',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--description "Fund the docs"',
            '--requested-amount 150',
            '--json',
        ])).stdout
        proposal_txid = json.loads(proposal)['proposalCell']['txHash']
        time.sleep(20)
        bash(' '.join([
            'bun run src/cli/index.ts vote',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            f'--proposal {proposal_txid}:0',
            '--direction yes',
        ]))
        time.sleep(20)
        counting = bash_output(' '.join([
            'bun run src/cli/index.ts create-counting',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            f'--proposal {proposal_txid}:0',
            '--direction yes',
            '--start-hash 0',
            '--end-hash 65535',
            '--wait',
            '--json',
        ])).stdout
        counting_txid = json.loads(counting)['countingCell']['txHash']
        time.sleep(20)
        finalize = bash_output(' '.join([
            'bun run src/cli/index.ts finalize-proposal',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            f'--proposal {proposal_txid}:0',
            f'--counting {counting_txid}:0',
            '--wait',
            '--json',
        ])).stdout
        finalize_txid = json.loads(finalize)['txHash']
        time.sleep(20)
        passpass = bash_output(' '.join([
            'bun run src/cli/index.ts pass-proposal',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            f'--proposal {finalize_txid}',
            '--wait',
            '--json',
        ])).stdout
        passpass_txid = json.loads(passpass)['txHash']
        time.sleep(20)
        bash(' '.join([
            'bun run src/cli/index.ts receive-grant',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            f'--proposal {passpass_txid}',
            '--json',
        ]))


def develop():
    subprocess.run('pm k ckbm ckbr', shell=True)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('rm -rf data ckb ckb-cli pk1 pk2')
    with cd('/home/ubuntu/.ckb-cli/keystore'):
        bash('rm -rf UTC*')
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('ln -s ~/app/ckb/bin/ckb .')
        bash('ln -s ~/app/ckb/bin/ckb-cli .')
        bash('echo 0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc > pk1')
        bash('echo 0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d > pk2')
        bash('./ckb-cli account import --privkey-path pk1')
        bash('./ckb-cli account import --privkey-path pk2')
        bash('PMNAME=ckbm pm c ./ckb run --indexer')
        bash('PMNAME=ckbr pm c ./ckb miner')
    time.sleep(20)
    pyckb.config.upgrade(pyckb.config.develop.rpc.url)
    pyckb.config.current = pyckb.config.develop
    gods = [
        pyckb.wallet.Wallet(0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc),
        pyckb.wallet.Wallet(0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d),
    ]
    user = [pyckb.wallet.Wallet(i) for i in range(1, 3)]
    gods[0].transfer(user[0].script, gods[0].capacity() // 2)
    gods[1].transfer(user[1].script, gods[1].capacity() // 2)
    time.sleep(20)
    for u in gods + user:
        u.dao_deposit(1000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/sdk'):
        bash('bun run src/cli/index.ts devnet-scripts --out ../deployment/devnet.json')
        bash(' '.join([
            'bun run src/cli/index.ts deploy',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--out ../deployment/devnet.json',
        ]))
        time.sleep(20)
        bash(' '.join([
            'bun run src/cli/index.ts create-config',
            '--config ../deployment/devnet.json',
            '--write ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--minimal-proposal-capacity 200',
            '--yes-threshold 1500',
            '--minimal-vote-amount 100',
            '--vote-duration 1000',
            '--vote-window 1000',
            '--challenge-time 1000',
        ]))


def vote256():
    subprocess.run('pm k ckbm ckbr', shell=True)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('rm -rf data ckb ckb-cli pk1 pk2')
    with cd('/home/ubuntu/.ckb-cli/keystore'):
        bash('rm -rf UTC*')
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/devnet'):
        bash('ln -s ~/app/ckb/bin/ckb .')
        bash('ln -s ~/app/ckb/bin/ckb-cli .')
        bash('echo 0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc > pk1')
        bash('echo 0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d > pk2')
        bash('./ckb-cli account import --privkey-path pk1')
        bash('./ckb-cli account import --privkey-path pk2')
        bash('PMNAME=ckbm pm c ./ckb run --indexer')
        bash('PMNAME=ckbr pm c ./ckb miner')
    time.sleep(20)
    pyckb.config.upgrade(pyckb.config.develop.rpc.url)
    pyckb.config.current = pyckb.config.develop
    gods = [
        pyckb.wallet.Wallet(0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc),
        pyckb.wallet.Wallet(0x63d86723e08f0f813a36ce6aa123bb2289d90680ae1e99d4de8cdb334553f24d),
    ]
    user = [pyckb.wallet.Wallet(i) for i in range(1, 257)]
    gods[0].transfer(user[0].script, 1280000 * pyckb.denomination.ckbytes)
    gods[1].transfer(user[1].script, 1280000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 2):
        user[i].transfer(user[i + 2].script, 640000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 4):
        user[i].transfer(user[i + 4].script, 320000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 8):
        user[i].transfer(user[i + 8].script, 160000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 16):
        user[i].transfer(user[i + 16].script, 80000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 32):
        user[i].transfer(user[i + 32].script, 40000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 64):
        user[i].transfer(user[i + 64].script, 20000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for i in range(0, 128):
        user[i].transfer(user[i + 128].script, 10000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    for u in gods + user:
        u.dao_deposit(1000 * pyckb.denomination.ckbytes)
    time.sleep(20)
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/sdk'):
        bash('bun run src/cli/index.ts devnet-scripts --out ../deployment/devnet.json')
        bash(' '.join([
            'bun run src/cli/index.ts deploy',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--out ../deployment/devnet.json',
        ]))
        time.sleep(20)
        bash(' '.join([
            'bun run src/cli/index.ts create-config',
            '--config ../deployment/devnet.json',
            '--write ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--minimal-proposal-capacity 200',
            '--yes-threshold 255000',
            '--minimal-vote-amount 100',
            '--vote-duration 1000',
            '--vote-window 1000',
            '--challenge-time 1000',
        ]))
        time.sleep(20)
        proposal = bash_output(' '.join([
            'bun run src/cli/index.ts create-proposal',
            '--config ../deployment/devnet.json',
            '--private-key-file ../devnet/pk1',
            '--description "Fund the docs"',
            '--requested-amount 150',
            '--json',
        ])).stdout
        proposal_txid = json.loads(proposal)['proposalCell']['txHash']
        time.sleep(20)
        for u in user:
            with open('../devnet/pk3', 'w+') as f:
                f.write(f'0x{u.prikey.hex()}')
            bash(' '.join([
                'bun run src/cli/index.ts vote',
                '--config ../deployment/devnet.json',
                '--private-key-file ../devnet/pk3',
                f'--proposal {proposal_txid}:0',
                '--direction yes',
            ]))


def testnet():
    pyckb.config.current = pyckb.config.testnet
    conf = json.loads(pathlib.Path('deployment/testnet.json').read_text())
    user = pyckb.wallet.Wallet(1)
    todo = {
        'alwaysSuccess': 'build/release/always-success',
        'config': 'build/release/config-type-script',
        'counting': 'build/release/counting-type-script',
        'proposal': 'build/release/proposal-type-script',
        'vote': 'build/release/vote-type-script',
    }
    for k, v in todo.items():
        data = bytearray(pathlib.Path(v).read_bytes())
        if k in conf['scripts']:
            print('update', k, v)
            out_point = pyckb.core.OutPoint(
                bytearray.fromhex(conf['scripts'][k]['cellDep']['txHash'][2:]),
                conf['scripts'][k]['cellDep']['index'],
            )
            hash = user.script_update_type_id(user.script, data, out_point)
            pyckb.rpc.wait(f'0x{hash.hex()}')
            conf['scripts'][k]['cellDep']['txHash'] = f'0x{hash.hex()}'
            continue
        print('deploy', k, v)
        hash = user.script_deploy_type_id(user.script, data)
        pyckb.rpc.wait(f'0x{hash.hex()}')
        kype_json = pyckb.rpc.get_transaction(f'0x{hash.hex()}')['transaction']['outputs'][0]['type']
        kype = pyckb.core.Script.rpc_decode(kype_json)
        conf['scripts'][k] = {
            'codeHash': f'0x{kype.hash().hex()}',
            'hashType': 'type',
            'args': '0x',
            'cellDep': {
                'txHash': f'0x{hash.hex()}',
                'index': 0,
                'depType': 'code'
            }
        }
    pathlib.Path('deployment/testnet.json').write_text(json.dumps(conf, indent=2))

    with open('devnet/pk3', 'w+') as f:
        f.write(f'0x{user.prikey.hex()}')
    with cd('/home/ubuntu/src/ckb-treasury-lab/solution1/sdk'):
        bash(' '.join([
            'bun run src/cli/index.ts create-config',
            '--config ../deployment/testnet.json',
            '--write ../deployment/testnet.json',
            '--private-key-file ../devnet/pk3',
            '--minimal-proposal-capacity 200',
            '--yes-threshold 1000',
            '--minimal-vote-amount 100',
            '--vote-duration 180',
            '--vote-window 180',
            '--challenge-time 180',
        ]))


if __name__ == '__main__':
    eval(f'{sys.argv[1]}()')
