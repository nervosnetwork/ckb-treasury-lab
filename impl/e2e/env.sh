# this is the address of pre-funded cells created by `ckb init --chain dev`, available only on the local devnet
FROM_ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqwgx292hnvmn68xf779vmzrshpmm6epn4c0cgwga"
CKB_RPC="http://127.0.0.1:8114"

E2E_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export CKB_CLI_HOME="$E2E_DIR/.ckb-cli"
