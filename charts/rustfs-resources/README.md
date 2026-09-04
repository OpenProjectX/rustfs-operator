# rustfs-resources Helm chart

Declare RustFS resources — `Bucket`, `Policy`, `User` custom resources —
from Helm values. One release per team/app/namespace; the
[rustfs-operator](https://github.com/OpenProjectX/rustfs-operator)
reconciles them against the RustFS server. Resources are created in the
release namespace.

```sh
helm repo add rustfs-operator https://openprojectx.github.io/rustfs-operator
helm install app-storage rustfs-operator/rustfs-resources \
  --namespace team-a -f my-resources.yaml
```

## Example

```yaml
# my-resources.yaml
connection:            # default for every entry; per-entry override possible
  clusterRef: prod     # or secretRef: <connection Secret in this namespace>

buckets:
  - name: app-data
    versioning: true
    quotaBytes: 10737418240      # 10 GiB
    deletionPolicy: Retain       # keep the bucket if the CR is deleted

policies:
  - name: app-data-rw
    document:
      Version: "2012-10-17"
      Statement:
        - Effect: Allow
          Action: ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket"]
          Resource: ["arn:aws:s3:::app-data", "arn:aws:s3:::app-data/*"]

users:
  - name: app-user
    policies: ["app-data-rw"]
    passwordRef:                 # existing Secret with key `password`
      name: app-user-creds

accessKeys:
  - name: app-key                # operator writes AK/SK to Secret
    user: app-user               # "app-key-credentials" in this namespace
```

## Values

| Key | Description |
|-----|-------------|
| `connection.clusterRef` / `connection.secretRef` | default connection (exactly one) |
| `buckets[]` | `name` (required), `bucketName`, `versioning`, `quotaBytes`, `deletionPolicy`, `connection` |
| `policies[]` | `name` (required), `document` (required), `policyName`, `deletionPolicy`, `connection` |
| `users[]` | `name` (required), `username`, `passwordRef` **or** inline `password`, `policies`, `enabled`, `deletionPolicy`, `connection` |
| `accessKeys[]` | `name`, `user` (required), `accessKey`, `description`, `policy`, `targetSecretName`, `deletionPolicy`, `connection` |

Fields you omit stay unmanaged (e.g. no `versioning` key means the operator
never touches versioning). `deletionPolicy` defaults to `Delete` — the
remote resource is removed when the CR is deleted; use `Retain` to keep it.

**User passwords**: `passwordRef` points at an existing Secret in the
release namespace. Alternatively set `password` inline and the chart
creates `<release>-user-<name>` — but it then lives in the Helm release
values; prefer `passwordRef` in production. Passwords are only applied when
the user is first created, so changing one later has no effect. Note the
username/password pair is itself a working S3 credential — an `accessKeys[]`
entry is only needed for per-consumer or reduced-scope credentials
(see [docs/iam-model.md](../../docs/iam-model.md)).

**Access keys**: each `accessKeys[]` entry issues an AK/SK pair owned by
`user`; the operator writes the generated credentials to a Secret (default
`<name>-credentials`). The key inherits that user's policies, so grant
access via `users[].policies` rather than per key.

Only `user` is needed: since chart 0.7.0 the operator issues keys with its own
admin credential, naming the owner via `targetUser`. `passwordFromUser` and
`passwordRef` were removed and are rejected at render time, and the owning
user no longer needs `admin:CreateServiceAccount` /
`admin:ListServiceAccounts` / `admin:RemoveServiceAccount`. This requires the
operator's connection to hold RustFS root — see
[docs/iam-model.md](../../docs/iam-model.md).

## Prerequisites

The operator and CRDs must be installed (main `rustfs-operator` chart), and
the referenced `ClusterConnection` or connection Secret must exist.
