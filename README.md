# rustfs-operator

A Kubernetes operator that manages **RustFS** resources declaratively:
buckets, IAM users and IAM policies. Built on [kube-rs](https://kube.rs) and
the [`rc-core`](https://crates.io/crates/rc-core) /
[`rc-s3`](https://crates.io/crates/rc-s3) client crates from
[rustfs/cli](https://github.com/rustfs/cli).

## CRDs (`rustfs.com/v1alpha1`)

| Kind     | Short name | Manages                                             |
|----------|------------|-----------------------------------------------------|
| `Bucket` | `rfb`      | bucket existence, versioning, hard quota            |
| `User`   | `rfu`      | IAM user, enabled/disabled, attached policies       |
| `Policy` | `rfp`      | IAM policy document (inline YAML/JSON)              |

Every resource references a connection Secret in its own namespace:

```yaml
stringData:
  endpoint: http://rustfs.storage.svc:9000
  accessKey: rustfsadmin
  secretKey: rustfsadmin
  # region: us-east-1   # optional
  # insecure: "true"    # optional
```

See `deploy/example.yaml` for a complete example. Each resource supports
`deletionPolicy: Delete` (default; the remote resource is removed via
finalizer when the CR is deleted) or `Retain`.

## Install & run

```sh
# CRDs (regenerate with: cargo run -- crd > deploy/crds.yaml)
kubectl apply -f deploy/crds.yaml
kubectl apply -f deploy/rbac.yaml

# run the controllers (in-cluster or with a local kubeconfig)
cargo run --release -- run
```

## Behavior notes

- **Reconcile loop**: finalizer-based; drift is re-checked every 5 minutes,
  errors retry after 15s and are reported in `.status.message`.
- **User secret keys** are only applied at user creation; RustFS does not
  expose secret keys, so rotating one requires deleting/recreating the user.
- **Policy attachment** uses RustFS's `set-user-or-group-policy` endpoint,
  which *replaces* the whole attachment set — `spec.policies` is therefore
  fully declarative.
- **Policy drift detection** compares documents semantically: the server
  normalizes stored policies (adds empty `Sid`/`Condition`, reorders string
  arrays, wraps in metadata), so byte-comparison would never converge.

## Testing

| Layer | Command | Needs |
|-------|---------|-------|
| Unit (mocked provider) | `cargo test` | – |
| Integration (real RustFS) | `cargo test --features integration --test integration_rustfs` | Docker, `rustfs/rustfs:1.0.0-beta.8` |
| E2E (real k3s + RustFS, controllers in-process) | `cargo test --features e2e --test e2e_k3s` | Docker, `rancher/k3s:v1.34.9-k3s1` |

The e2e test boots a k3s cluster and a RustFS server in containers, installs
the CRDs, runs the controllers inside the test process, applies
Bucket/User/Policy CRs and asserts both convergence in RustFS and finalizer
cleanup on deletion.
