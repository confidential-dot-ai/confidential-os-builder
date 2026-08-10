# c8s multinode (design)

Status: **draft**. Extends the single-node posture in
[C8S-IMAGE.md](C8S-IMAGE.md) to one rke2 server plus N agents, each its own
TDX CVM on its own machine. `c8s install --distro rke2` stays unchanged.

## Problem

The single-node image caps a c8s deployment at one machine's resources;
multi-machine workloads (cross-node inference, data-parallel jobs) need N
nodes in one cluster. Naive multinode (a join token on a config disk) hands
cluster membership to the untrusted host, so scaling out is blocked on an
attestation story for join. Success = validation stages M0 to M4 below.

Out of scope for this iteration: HA control plane (3 etcd servers),
persistent etcd across reboots, mixed-measurement clusters (see upgrade
posture below).

## Threat model

Attacker: the host under every node, and the network between nodes. They
control all unmeasured disks (config drop-ins, scratch, containerd cache),
all inter-node traffic, and boot timing. They can also capture and replay
protocol messages, including relaying attestation evidence produced by a
live CVM elsewhere. They want to (a) join a node they control into the
cluster, (b) read or modify cross-node traffic, (c) steal the join
credential and reuse it later.

Trust anchor: the TDX launch measurement of the one published image, plus
attestation-api quotes at runtime. A corollary that shapes everything
below: **any value that arrives via NoCloud meta-data or a config drop-in
disk is host-controlled** and may only ever cause denial of service, never
widen admission.

Invariant classes: the join components (`join-release`, `c8s join`) are
correctness-class (auth over an attested channel: simplest auditable
implementation, verify per request, no caching of verification results,
fail closed). Role dispatch is simplicity-class. The WG datapath is the
one speed-relevant piece and gets measured (M3), not assumed.

## Existing solutions considered

- **Constellation (Edgeless Systems)**: CVM Kubernetes with exactly this
  shape: a JoinService gating node join on attestation with a same-image
  policy, plus a WireGuard/Cilium-encrypted node network. Validates the
  design direction, but it is a whole distribution (own bootstrapper,
  own image pipeline, own CLI); adopting it replaces the measured-confos +
  rke2 + c8s stack rather than extending it.
- **CoCo Trustee / KBS** (the org already maintains `coco-kbs-go`): the
  join token could be a KBS resource gated on an attestation policy. The
  closest drop-in alternative. Rejected for v1: KBS assumes an external
  relying-party deployment whose policy lives outside the guest
  measurement, while `cred-release` plumbing (RA-TLS listener, quote
  verification, local attestation-api client) is already baked into this
  image and measured; the join service is a thin sibling of it. Revisit if
  join policy ever needs to be operator-tunable rather than
  measured-by-construction.
- **SPIRE/SPIFFE with a TDX node attestor**: issues workload/node
  identities from attestation, but does not speak rke2's token bootstrap;
  it would add a server+agent infrastructure component inside the trust
  boundary to gate one boot-time RPC. Disproportionate.
- **Keylime**: TPM-centric remote attestation; wrong evidence type for
  TDX quotes here.
- **rke2 natively**: has no attestation hook on join; the shared token is
  the only gate. That is the gap this design fills.

## Design

### One image, runtime role selection

One published image, one measurement, boots as server or agent. Two images
would double the measurement surface and force `cds.measurements` to carry
both. Role is selected at runtime:

- A small oneshot (`rke2-role.service`, baked) reads an optional disk
  labelled `joindata` (files: `role`, `server`; attached like `opkeydata`)
  and writes `/run/confos/role-server` or `role-agent`, plus the server
  address for the join client. No disk defaults to server, which preserves
  today's single-node behavior exactly.
- Both rke2 units stay preset-enabled; baked drop-ins add
  `ConditionPathExists=/run/confos/role-server` (server) and
  `role-agent` (agent). Strictly either/or per node, never both, which
  also keeps rke2-server's existing agent-is-active ExecCondition happy.

Role is host-controlled and that is fine: flipping a node to agent without
a gated join credential just fails to join; flipping to server forms an
isolated empty cluster nobody trusts (its CDS has no workloads, and
operators only talk to nodes they verify). DoS only.

`cred-release.service` additionally gains
`ConditionPathExists=/run/confos/role-server`: today it gates only on the
opkeydata disk, and an operator key passed to an agent boot would wait ten
minutes for a client CA that never appears.

### Attestation-gated join (the core piece)

rke2 joins agents with a bearer token. Putting that token on the drop-in
disk hands it to the host, and a host holding the token can join a
non-CVM node straight into kubelet/etcd trust. So the token never touches
persistent or host-visible storage; it is released only across an
attested channel, in both directions:

- **Server side**: a new baked `join-release.service` on the server
  (`c8s join-release`, sibling of `cred-release`, RA-TLS on `:8444`).
  It serves the cluster join token after verifying the caller's TDX quote.
- **Agent side**: a new baked `rke2-join.service` oneshot (agent role
  only). It runs `c8s join --server <addr>:8444`, which verifies the
  server's RA-TLS cert against the server's TDX quote in-process (same
  mechanics as `c8s get-kubeconfig`), presents the agent's own quote, and
  receives the token. `rke2-agent` gets a baked drop-in with
  `Requires=rke2-join.service` + `After=` so it cannot start without a
  successful join: fail closed by unit dependency, not by accident.

**Both quotes are channel-bound.** RA-TLS binds each side's quote to the
TLS certificate key actually presented on the wire (`report_data` carries
the cert key hash, the same binding `c8s get-kubeconfig` already verifies
server-side). `c8s join` adds the client direction: it generates an
ephemeral client cert, obtains a quote over its key hash from the local
attestation-api, and presents both; `join-release` verifies quote
signature, key binding, and measurement policy before releasing. A quote
lifted from a live CVM cannot be replayed by a non-TEE client: its
`report_data` will not match the client key on the wire. Freshness comes
from the binding (the private key never leaves the guest), not from
wall-clock nonces.

**Peer policy is same-image, not a configured list**, and the register
set is exact: `mrtd`, `rtmr[1]`, `rtmr[2]` must equal the verifier's own
values, read via the local attestation-api. This is the same register set
`manifest.json` publishes as the image's reference values. Excluded
deliberately:

- `rtmr[3]`: runtime register, asymmetric by design (operator boots
  extend the operator pubkey into it; agent boots leave it empty).
  Requiring equality would break every operator-boot server ↔ agent join.
- `rtmr[0]`: carries launch-configuration inputs that legitimately vary
  per host and launch shape; it is likewise excluded from the
  `manifest.json` reference values.

The policy is therefore measured by construction: no operator-supplied
measurement list sits on an unmeasured disk for the host to edit. The
fleet is exactly "nodes booted from this image".

**API contract** (flat route, no version prefix, per c8s convention):

```
GET /join-token          mutual RA-TLS, no request body
  200  {"token": "K10<server-ca-hash>::agent:<secret>"}
  403  measurement policy mismatch (logged: verdict + peer measurements,
       never the token)
  503  token not yet readable (server still initialising)
```

Request handling is bounded (read limits, per-request timeout,
configurable); verification runs per request with no caching of verdicts.

**Token handling on the agent**: `c8s join` writes the token to tmpfs
(`/run/confos/join-token`) and points rke2 at it via a
`token-file:` config fragment, together with the `server:
https://<addr>:9345` line. The released token is rke2's full format,
which embeds the server CA hash, so rke2's own TLS bootstrap pins the
server CA after the attested exchange (validated in M2 below). The server
address on the drop-in disk stays host-controlled: redirecting it fails
the RA-TLS quote check, DoS only.

Distributed-systems posture, per the engineering standards:

- Consistency: cluster membership is linearizable through the single
  server's etcd; join-release reads the token from local disk, no
  cross-node state.
- Idempotency: token release is a read, safely repeatable; rke2 node
  registration is idempotent by node name.
- Timeouts: `c8s join` uses a bounded per-attempt timeout (flag, default
  30s); the unit retries indefinitely, spaced out, and `Upholds=` re-pulls
  rke2-agent once the join lands (a failed first attempt cancels the
  agent's queued start job for good). `join-release` keeps a bounded
  StartLimit: its persistent failure is a broken local attestation stack.
- Partition: an agent that cannot reach the server never joins and runs
  nothing. Already-joined agents ride out partitions with kubelet's own
  reconnect; workloads keep running, no split-brain is possible with a
  single server.
- Clocks: no new clock assumption. The join exchange is fresh via key
  binding, not wall-clock based; timesyncd already covers cert-validity
  skew.

Known gap, deliberate: two deployments of the same image are mutually
attested, so a host could point a new agent at a *different* deployment's
server (cross-cluster splice). Both clusters are attested and
operator-owned, so this moves capacity, not trust. If it matters, the fix
is a launch-bound cluster ID extended into RTMR[3] by the initrd (the
operator-key mechanism already does exactly this) with join-release
requiring RTMR[3] equality; note that re-includes rtmr[3] in the policy
and must compose with the operator-key extend (ordered, deterministic).
Deferred; see open questions.

### Cross-node datapath encryption

Control-plane traffic (kubelet, apiserver, etcd, supervisor) is mTLS
already. Pod-to-pod traffic between nodes is not: today it would cross
the untrusted fabric as cleartext VXLAN. Changes:

- Enable Cilium WireGuard node-to-node encryption in the baked
  HelmChartConfig (`encryption: {enabled: true, type: wireguard}`).
  WG public keys are distributed via CiliumNode objects, so peer trust
  chains to apiserver membership, which the join gate above makes
  attestation-equivalent. This is why join gating is a prerequisite,
  not a parallel track.
- Failure semantics must be drop, not plaintext fallback: M2 asserts that
  cross-node pod traffic to a peer with no WG key is dropped, and that no
  pod-to-pod bytes appear in cleartext on the host bridge at any point
  including Cilium restarts.
- Kernel: add `CONFIG_WIREGUARD=y` to `kernel/c8s.config`
  (`CONFIG_VXLAN`/`CONFIG_GENEVE` are already `=y`; XFRM/IPsec stays
  out). Everything must be `=y`, `modules_disabled=1` as usual.
  Regenerate the snapshot in the same PR.
- MTU: VXLAN plus WireGuard costs ~130 bytes of overhead; let Cilium's
  auto-MTU handle it and assert pod-to-pod MTU sanity in validation
  rather than hand-setting it.
- Throughput is a real cost for cross-node GPU traffic (NCCL over
  TCP rides this path). M3 measures cross-node iperf and NCCL bandwidth
  through WG on real hardware before any workload commitment; if it is
  unacceptable, the fallback discussion (dedicated interconnect, GPUDirect
  paths outside the pod network) happens then, with numbers.

Not covered: host-namespace traffic that is neither rke2 mTLS nor pod
traffic (e.g. a NodePort hop's second leg). Reviewed in validation;
Cilium's node-encryption mode is not assumed.

### The `k8sServiceHost: 127.0.0.1` question

The baked Cilium values point the agent at the apiserver on loopback,
which today only works because the server is local. rke2 agents run a
client-side apiserver load balancer on loopback, which should make the
same value work on agent nodes unverified-but-likely; M1 checks it
first thing. Fallback if wrong: point `k8sServiceHost` at the server's
registration address via the same runtime config path as the join
fragment (HelmChartConfig is cluster-wide, so a per-role override has to
ride the drop-in mechanism, not the baked manifest).

### Inter-node networking (launch/infra layer, not the image)

The single-node image runs behind masquerade NAT (egress only). Multinode
requires **routable node addresses**, inbound included; that is launch
tooling work, not image work. Port matrix every deployment must open
between nodes:

| Port | Proto | Dir | What |
|---|---|---|---|
| 9345 | tcp | agent → server | rke2 supervisor (join, bootstrap) |
| 6443 | tcp | agent → server | kube-apiserver |
| 8444 | tcp | agent → server | join-release (RA-TLS) |
| 8472 | udp | node ↔ node | VXLAN overlay |
| 51871 | udp | node ↔ node | WireGuard |
| 30808 | tcp | node ↔ node | CDS NodePort (allowlist serve) |
| 8443 | tcp | operator → server | cred-release |

`node-ip` and `node-external-address` land in the per-host drop-in like
everything else per-host. CIDR posture is unchanged: rke2 defaults
10.42/10.43, the 10.52/10.53 inner-cluster reservation from
C8S-IMAGE.md still applies, and a /16 pod CIDR covers any realistic N.

### c8s on top

Untouched, by construction: installer, attestation-api and NRI config
distribution are DaemonSets, and each node bakes its own attestation-api
+ NRI plugin + `nri-node-ip.service`. Notes:

- **Installer restart unit**: already role-agnostic. The chart's
  `nri-image-policy.restartCommand` restarts whichever of
  `rke2-server`/`rke2-agent` is active on the node. No change needed.
- **CDS placement**: single replica pinned by the `role=cds` label,
  unchanged. Every node's baked NRI plugin dials
  `https://127.0.0.1:30808`, which on the single-node image has always
  been node-local. On a multinode cluster the off-CDS nodes' NodePort hop
  must actually traverse to the CDS node; kubeProxyReplacement should
  route it, but this is the c8s-side assumption most likely to be wrong,
  so M1 verifies it explicitly (and open question 5 covers whether that
  leg rides the WG path).
- **Measurements**: one image means one measurement set; the existing
  `cds.measurements` values cover every node with no schema change.
- **Mesh**: ratls-mesh is node-scoped and derives identity per node
  already; no change expected, exercised in M1.

### Lifecycle posture (v1.5: still ephemeral)

Reboot of the **server** = the cluster is gone; agents retry a dead
server forever. The supported recovery is relaunch the fleet, then
`c8s install` again, same as today's single-node reprovision, now N
machines wide. Scale-out is boot-an-agent (drop-in with role + server
address, attested join, node Ready). Scale-in is delete the node object
and kill the VM.

Upgrade and rollback follow from the same-image join policy: a
new-measurement image cannot join an old-measurement cluster, so there
are **no rolling image upgrades**; upgrade = relaunch the fleet on the
new image, rollback = relaunch on the previous one (`manifest.json` per
version is the reference). This is the ephemerality posture applied to
upgrades, stated so nobody designs a mixed-fleet migration against it.
Persistent/HA etcd stays out of scope until untrusted-host disk
integrity for etcd has a story.

## Deltas summary

| Where | Change | Rough size |
|---|---|---|
| confos c8s profile | `rke2-role.service` + role conditions on rke2 units and cred-release; `join-release.service`; `rke2-join.service`; Cilium WG values | ~150 lines of units/config |
| confos kernel | `CONFIG_WIREGUARD=y` in `kernel/c8s.config` + snapshot regen | 1 line + snapshot |
| c8s repo | `c8s join-release` + `c8s join` subcommands sharing cred-release's RA-TLS/quote plumbing | ~400 lines Go + tests |
| launch tooling | routable node addressing; port matrix; per-host role/server drop-ins | deployment-repo scoped |

No new dependencies: the join path reuses the ratls + attestation-api
client code cred-release already links.

## Validation stages

- **M0 units**: role dispatch (no file → server; agent file → agent, no
  server unit); join client and release server against recorded quotes;
  same-image accept, different-measurement reject, wrong-key-binding
  reject.
- **M1 two VMs, GPU-less, one host**: server + agent boot, attested
  join, both Ready. Cilium healthy across nodes with WG
  (`cilium status` shows encryption on; tcpdump on the host bridge shows
  only WG frames for pod-to-pod). `k8sServiceHost` loopback verified on
  the agent. `c8s install`: one rke2 unit restart per node, CDS on the
  labeled node, an unlisted image denied **on the agent node**, mesh
  functional across nodes.
- **M2 adversarial**: token never on persistent storage (image + scratch
  grep); a non-CVM client replaying the agent's drop-in disk is refused;
  a relayed same-image quote with mismatched key binding is refused; an
  agent pointed at a rogue endpoint refuses to join; released token's
  CA-hash pinning verified against a wrong-CA server; cross-node pod
  traffic to a WG-keyless peer drops rather than falling back to
  cleartext.
- **M3 two TDX machines with GPUs**: M1 exit criteria on real hardware
  plus GPU pods scheduled on both nodes; measurements from
  `manifest.json` verify on both; cross-node iperf + NCCL bandwidth
  through WG measured and recorded.
- **M4 scale**: add a third node to a running cluster; kill it;
  server reboot drill ends in documented full-relaunch recovery.

## Open questions

1. Cross-cluster splice: is the RTMR[3] cluster-ID binding worth doing
   now, or acceptable to defer while all deployments are one-cluster?
2. `k8sServiceHost` on agents: confirm the rke2 agent loopback LB serves
   6443 (M1); pick the fallback if not.
3. Does the released full-format token's CA-hash check behave as assumed
   during rke2 bootstrap (M2)?
4. HA control plane: when it lands, join-release must run on every
   server and agents need a server list; the same-image policy already
   covers it, but etcd quorum vs the ephemerality posture does not.
5. NodePort second-leg traffic (client → nodeA → CDS on nodeB): confirm
   the inter-node leg rides the WG path under kubeProxyReplacement.
6. The baked `config.yaml` carries server-only keys; confirm rke2-agent
   treats them as warnings, not fatal (M1).
6. WG throughput on TDX hardware (M3 numbers): acceptable for cross-node
   GPU workloads, or does multinode need a datapath exception with its
   own protection story?
