# CKB Treasury Web

A React application built with Bun and Vite on top of the voting SDK in `../sdk`. It supports local CKB devnet and public CKB testnet, with direct ckb-tw and CCC/Khie wallet connections.

## Wallets

Choose **Connect wallet**, then **ckb-tw** for the browser extension or **CCC / Khie** for the CCC wallet selector. Choose **Khie** there to pair by QR code or pairing link. Reopen the wallet menu to disconnect or switch wallets.

The selected wallet connection is restored after a page refresh when the wallet remains available and authorized. Disconnecting clears the saved selection.

The connector uses the selected deployment's network and RPC. Khie wallets must support CKB transaction signing and the account's lock on that network. Voting requires eligible DAO deposits under the same account lock and funds for transaction fees. Devnet wallets must also support that devnet's deployed lock scripts.

`@ckb-ccc/connector-react` is pinned to `2.1.3` (Connector `2.2.1`) to share CCC core `1.21.0` with the SDK's shell `1.3.12`. Update these packages together to avoid incompatible signer/client classes.

## Network Selection

Set `CKB_VOTE_NETWORK` when starting Vite or building the application:

|       Value        |       Deployment file        | Displayed network |
| ------------------ | ---------------------------- | ----------------- |
| `devnet` (default) | `../deployment/devnet.json`  | Develop network   |
| `testnet`          | `../deployment/testnet.json` | Testnet           |

Run these commands from this directory:

```sh
$ bun install --registry https://registry.npmjs.org
$ CKB_VOTE_NETWORK=testnet bun run dev
# Or
$ CKB_VOTE_NETWORK=devnet  bun run dev
```

To run the application on devnet, follow `../docs/knowledge/devnet.md` to set up your local environment.

## Docker

Build and run the development server from the `solution1` directory:

```sh
$ docker build -f web/Dockerfile -t ckb-treasury-lab-web .
$ docker run --rm -p 5173:5173 ckb-treasury-lab-web
```

The image uses Ubuntu 24.04, installs Bun, and runs `bun run dev` from
`web`. It defaults to testnet; set `CKB_VOTE_NETWORK=devnet` when running the
container if `deployment/devnet.json` is available.
