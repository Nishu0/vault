#!/usr/bin/env bash
# Snapshot the deployed contracts for Soroquake fork mode.
#
#   scripts/snapshot.sh            # testnet, latest archived ledger
#
# Writes snapshots/testnet.json (ledger state + contract wasm) and
# snapshots/testnet.meta.json (contract and actor names).
set -euo pipefail
cd "$(dirname "$0")/.."
NETWORK="${NETWORK:-testnet}"
# Highest protocol the soroban-sdk test host supports. SDK 30 matches testnet,
# so snapshots replay at the network's real protocol.
HOST_PROTOCOL="${HOST_PROTOCOL:-30}"
VAULT="${VAULT:-CACRZSIZCR5ZGC5LT5PER6O4HMGBDLX744O4D5FW77BNLXEF6AXI47SC}"
TOKEN="${TOKEN:-CDGQWHULGU4PYK7ZEPQFCCR6QATJVXPW3MLUI755HV65XAEI3WXSQWLT}"
ALICE="${ALICE:-GAOOZ3KQVY3QKX5BU5FCBCXMXI7FBSLFXUTG3ZO74MP7TBEF74TAYNG2}"
BOB="${BOB:-GDQB7VEPW77YKZCKF3P7WRC5HZOZHDOYXPOZ6XXMJWN2PU46AGPMGSA4}"
out="snapshots/$NETWORK.json"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p snapshots
stellar snapshot create --address "$VAULT" --address "$TOKEN" --network "$NETWORK" --out "$out"

# The archive snapshot holds contract storage; add the deployed wasm so the
# real code runs in fork mode.
for id in "$VAULT" "$TOKEN"; do
  stellar contract fetch --id "$id" --network "$NETWORK" --out-file "$tmp/$id.wasm"
done

python3 - "$out" "$tmp" "$VAULT" "$TOKEN" "$ALICE" "$BOB" "$NETWORK" "$HOST_PROTOCOL" <<'PY'
import hashlib, json, sys
out, tmp, vault, token, alice, bob, network, host_protocol = sys.argv[1:]
snap = json.load(open(out))
network_protocol = snap["protocol_version"]
snap["protocol_version"] = min(network_protocol, int(host_protocol))
have = {e["entry"]["data"].get("contract_code", {}).get("hash") for e in snap["ledger_entries"]}
for cid in (vault, token):
    code = open(f"{tmp}/{cid}.wasm", "rb").read()
    h = hashlib.sha256(code).hexdigest()
    if h in have:
        continue
    snap["ledger_entries"].append({
        "entry": {
            "last_modified_ledger_seq": snap["sequence_number"],
            "data": {"contract_code": {"ext": "v0", "hash": h, "code": code.hex()}},
            "ext": "v0",
        },
        "live_until": 4294967295,
    })
    have.add(h)
json.dump(snap, open(out, "w"), indent=1)
meta = {"network": network, "network_protocol": network_protocol, "host_protocol": snap["protocol_version"], "contracts": {"vault": vault, "token": token}, "actors": {"alice": alice, "bob": bob}}
json.dump(meta, open(out.replace(".json", ".meta.json"), "w"), indent=2)
print(f"{out}: ledger {snap['sequence_number']}, {len(snap['ledger_entries'])} entries, network protocol {network_protocol}, replayed on {snap['protocol_version']}")
PY
