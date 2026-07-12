# Feature request: optionally create ClusterConnection(s) + admin Secret from Helm values

## Context
The `rustfs-operator` chart currently deploys only the operator Deployment + RBAC.
The admin credentials Secret and the cluster-scoped `ClusterConnection` CR must be
created by hand (see `deploy/example.yaml`). In GitOps/helmfile setups this forces a
second, out-of-chart mechanism just to bootstrap the connection.

Add **optional** support to the chart so a `ClusterConnection` (and, optionally, its
admin credentials Secret) can be declared inline in `values.yaml`. Feature must be
**off by default** and must not change any existing rendered output when unset.

## Chart facts to respect
- CRD: group `rustfs.com`, version `v1alpha1`, kind `ClusterConnection`, **cluster-scoped**.
- `ClusterConnection.spec`: `endpoint` (required), `credentialsSecretRef` (required — name
  of a Secret **in the operator's namespace** with keys `accessKey`/`secretKey`),
  optional `region`, `insecure`, `allowedNamespaces` (string list; absent = all).
- The credentials Secret must live in the operator's own namespace (`.Release.Namespace`);
  a namespaced Role already grants the operator read access there.
- `rbac.clusterWideSecrets` already exists; when only `ClusterConnection` is used it can be
  `false`. Do not change its default.

## Values API (new)
Add a top-level `clusterConnections` **list** (support zero or more servers):

```yaml
clusterConnections: []
# clusterConnections:
#   - name: prod                     # required; metadata.name of the ClusterConnection (cluster-scoped)
#     endpoint: http://rustfs-svc.storage.svc.cluster.local:9000   # required
#     allowedNamespaces: []          # optional; omit/empty -> render nothing (operator treats absent = all)
#     region: ""                     # optional
#     insecure: false                # optional
#     credentials:
#       # Provide EITHER inline creds (chart creates the Secret in the operator ns) ...
#       create: true                 # default true
#       secretName: ""               # optional override; default: "<fullname>-<name>-admin"
#       accessKey: ""
#       secretKey: ""
#       # ... OR reference an existing Secret (chart creates no Secret):
#       existingSecret: ""           # if set, credentials.create is ignored and this name
#                                    # is used as credentialsSecretRef (must have accessKey/secretKey)
```

### Behavior
- `clusterConnections: []` (default) => render nothing new; byte-for-byte identical output to today.
- For each entry:
  - Always render a `ClusterConnection` (cluster-scoped, no namespace) with `spec.endpoint`,
    `spec.credentialsSecretRef` = resolved secret name (see below), and — only when set —
    `region`, `insecure`, and `allowedNamespaces`.
  - Resolve `credentialsSecretRef`:
    - if `credentials.existingSecret` is non-empty => use it; render **no** Secret.
    - else if `credentials.create` (default true) => render a `Secret` of `type: Opaque` in
      `.Release.Namespace` named `credentials.secretName | default "<fullname>-<name>-admin"`,
      with `stringData.accessKey` / `stringData.secretKey`; use that name as `credentialsSecretRef`.
  - `allowedNamespaces`: omit the field entirely when the list is empty/unset (so operator's
    "absent = all" semantics apply); render the list when provided.

## Validation (fail template with a clear message)
- `name` and `endpoint` are required per entry.
- Exactly one credentials source: reject if `existingSecret` is set **and** inline
  `accessKey`/`secretKey` are also set.
- When `credentials.create` is true and `existingSecret` is empty, both `accessKey` and
  `secretKey` must be non-empty (do not silently emit an empty-keyed Secret).
- Duplicate `name` values across the list should fail (they'd collide on a cluster-scoped object).

## Templates / conventions
- New file `templates/clusterconnection.yaml` (range over `.Values.clusterConnections`,
  `---` separated).
- Use existing `_helpers.tpl` fullname/labels helpers; apply the same standard labels the
  other objects use.
- CRDs in `crds/` are applied by Helm before `templates/`, so the CR renders fine on a fresh
  install — but please confirm ordering on first `helm install` (no CRD-not-found race).

## RBAC note
No change to defaults. Please document that when a deployment uses **only** ClusterConnection
(no per-namespace `secretRef`, no `User.secretKeyRef` in app namespaces), users can set
`rbac.clusterWideSecrets: false` for least privilege.

## Docs
- Document the new `clusterConnections` values block in the chart README with one inline-creds
  example and one `existingSecret` example.
- Note the security trade-off: inline `accessKey`/`secretKey` land in the release values;
  recommend `existingSecret` (or a sealed/external secret) for production.

## Acceptance criteria
1. `helm lint` passes; `helm template` with default values produces **no** ClusterConnection/Secret
   and is unchanged from current output.
2. `helm template` with one inline-creds entry renders: a Secret in the release namespace
   (keys `accessKey`/`secretKey`) **and** a ClusterConnection whose `credentialsSecretRef`
   equals that Secret's name.
3. Same with `existingSecret` set renders the ClusterConnection referencing that name and
   **no** Secret.
4. Multiple entries render multiple independent ClusterConnections/Secrets.
5. `allowedNamespaces` present only when provided.
6. Validation failures produce actionable error messages.
7. Bump chart `version`; update README.
